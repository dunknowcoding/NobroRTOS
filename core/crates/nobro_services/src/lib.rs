//! Optional, allocation-free application services.
//!
//! No service is enabled by default, and `nobro-nano` does not depend on this
//! crate. Applications select only the filesystem, USB-host, display, or shell
//! contracts they use.
#![no_std]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceInstanceId(u16);

impl ServiceInstanceId {
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceState {
    Down,
    Starting,
    Ready,
    Quiesced,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceError {
    InvalidConfig,
    InvalidIdentity,
    NotReady,
    Busy,
    Full,
    DeadlineElapsed,
    BackendFault,
}

#[cfg(feature = "filesystem")]
pub mod filesystem {
    //! Power-fail-safe fixed-capacity filesystem.

    pub use nobro_storage::{
        AtomicFileSystem, FileCommitReceipt, FileMetadata, FileSystemError, FileSystemMountError,
    };
}

#[cfg(feature = "usb-host")]
pub mod usb_host {
    use super::{ServiceError, ServiceInstanceId, ServiceState};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct UsbHostCapabilities {
        pub backend_id: &'static str,
        pub max_devices: u16,
        pub max_transfer_bytes: u16,
        pub control_pipe_bytes: u16,
        pub supports_hubs: bool,
    }

    impl UsbHostCapabilities {
        pub fn valid(self) -> bool {
            !self.backend_id.is_empty()
                && self.max_devices != 0
                && self.max_transfer_bytes != 0
                && self.control_pipe_bytes >= 8
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct UsbDevice {
        pub address: u8,
        pub vid: u16,
        pub pid: u16,
        pub class: u8,
        pub subclass: u8,
        pub protocol: u8,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct UsbHostMountReceipt {
        pub instance: ServiceInstanceId,
        pub capabilities: UsbHostCapabilities,
        pub lifecycle_generation: u32,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct UsbTransferReceipt {
        pub address: u8,
        pub endpoint: u8,
        pub bytes: u16,
    }

    pub trait UsbHostBackend {
        fn capabilities(&self) -> UsbHostCapabilities;
        fn state(&mut self) -> ServiceState;
        fn mount(&mut self) -> Result<(), ServiceError>;
        fn enumerate(&mut self, output: &mut [UsbDevice]) -> Result<usize, ServiceError>;
        fn transfer(
            &mut self,
            address: u8,
            endpoint: u8,
            output: &[u8],
            input: &mut [u8],
            now_us: u64,
            deadline_us: u64,
        ) -> Result<usize, ServiceError>;
        fn quiesce(&mut self) -> Result<(), ServiceError>;
        fn recover(&mut self) -> Result<(), ServiceError>;
    }

    pub struct UsbHostMountError<B> {
        backend: B,
        error: ServiceError,
    }

    impl<B> UsbHostMountError<B> {
        pub const fn error(&self) -> ServiceError {
            self.error
        }

        pub fn into_backend(self) -> B {
            self.backend
        }
    }

    pub struct MountedUsbHost<B> {
        backend: B,
        capabilities: UsbHostCapabilities,
    }

    impl<B: UsbHostBackend> MountedUsbHost<B> {
        pub fn mount(
            instance: ServiceInstanceId,
            mut backend: B,
        ) -> Result<(Self, UsbHostMountReceipt), UsbHostMountError<B>> {
            let capabilities = backend.capabilities();
            if !capabilities.valid() {
                return Err(UsbHostMountError {
                    backend,
                    error: ServiceError::InvalidIdentity,
                });
            }
            if let Err(error) = backend.mount() {
                return Err(UsbHostMountError { backend, error });
            }
            if backend.state() != ServiceState::Ready {
                return Err(UsbHostMountError {
                    backend,
                    error: ServiceError::BackendFault,
                });
            }
            Ok((
                Self {
                    backend,
                    capabilities,
                },
                UsbHostMountReceipt {
                    instance,
                    capabilities,
                    lifecycle_generation: 1,
                },
            ))
        }

        pub fn enumerate(&mut self, output: &mut [UsbDevice]) -> Result<usize, ServiceError> {
            if self.backend.state() != ServiceState::Ready {
                return Err(ServiceError::NotReady);
            }
            if output.len() > usize::from(self.capabilities.max_devices) {
                return Err(ServiceError::InvalidConfig);
            }
            let count = self.backend.enumerate(output)?;
            if count > output.len() || count > usize::from(self.capabilities.max_devices) {
                return Err(ServiceError::BackendFault);
            }
            Ok(count)
        }

        pub fn transfer(
            &mut self,
            address: u8,
            endpoint: u8,
            output: &[u8],
            input: &mut [u8],
            now_us: u64,
            deadline_us: u64,
        ) -> Result<UsbTransferReceipt, ServiceError> {
            if self.backend.state() != ServiceState::Ready {
                return Err(ServiceError::NotReady);
            }
            let limit = usize::from(self.capabilities.max_transfer_bytes);
            if address == 0
                || (output.is_empty() && input.is_empty())
                || output.len() > limit
                || input.len() > limit
            {
                return Err(ServiceError::InvalidConfig);
            }
            if deadline_us <= now_us {
                return Err(ServiceError::DeadlineElapsed);
            }
            let bytes =
                self.backend
                    .transfer(address, endpoint, output, input, now_us, deadline_us)?;
            if bytes > input.len() || bytes > usize::from(u16::MAX) {
                return Err(ServiceError::BackendFault);
            }
            Ok(UsbTransferReceipt {
                address,
                endpoint,
                bytes: bytes as u16,
            })
        }

        pub fn quiesce(&mut self) -> Result<(), ServiceError> {
            self.backend.quiesce()?;
            if self.backend.state() != ServiceState::Quiesced {
                return Err(ServiceError::BackendFault);
            }
            Ok(())
        }

        pub fn recover(&mut self) -> Result<(), ServiceError> {
            self.backend.recover()?;
            if self.backend.state() != ServiceState::Ready {
                return Err(ServiceError::BackendFault);
            }
            Ok(())
        }

        pub fn into_backend(self) -> B {
            self.backend
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        struct Fake {
            state: ServiceState,
        }

        impl UsbHostBackend for Fake {
            fn capabilities(&self) -> UsbHostCapabilities {
                UsbHostCapabilities {
                    backend_id: "test-usb-host",
                    max_devices: 1,
                    max_transfer_bytes: 16,
                    control_pipe_bytes: 64,
                    supports_hubs: false,
                }
            }

            fn state(&mut self) -> ServiceState {
                self.state
            }

            fn mount(&mut self) -> Result<(), ServiceError> {
                self.state = ServiceState::Ready;
                Ok(())
            }

            fn enumerate(&mut self, output: &mut [UsbDevice]) -> Result<usize, ServiceError> {
                if let Some(device) = output.first_mut() {
                    *device = UsbDevice {
                        address: 1,
                        vid: 0x1209,
                        pid: 1,
                        class: 0,
                        subclass: 0,
                        protocol: 0,
                    };
                    Ok(1)
                } else {
                    Ok(0)
                }
            }

            fn transfer(
                &mut self,
                _address: u8,
                _endpoint: u8,
                _output: &[u8],
                input: &mut [u8],
                _now_us: u64,
                _deadline_us: u64,
            ) -> Result<usize, ServiceError> {
                let reply = b"ok";
                input[..reply.len()].copy_from_slice(reply);
                Ok(reply.len())
            }

            fn quiesce(&mut self) -> Result<(), ServiceError> {
                self.state = ServiceState::Quiesced;
                Ok(())
            }

            fn recover(&mut self) -> Result<(), ServiceError> {
                self.state = ServiceState::Ready;
                Ok(())
            }
        }

        #[test]
        fn mount_enumerate_transfer_and_lifecycle_are_bounded() {
            let (mut host, receipt) = MountedUsbHost::mount(
                ServiceInstanceId::new(2),
                Fake {
                    state: ServiceState::Down,
                },
            )
            .ok()
            .unwrap();
            assert_eq!(receipt.instance.get(), 2);
            let empty = UsbDevice {
                address: 0,
                vid: 0,
                pid: 0,
                class: 0,
                subclass: 0,
                protocol: 0,
            };
            let mut devices = [empty; 1];
            assert_eq!(host.enumerate(&mut devices), Ok(1));
            let mut input = [0u8; 8];
            assert_eq!(
                host.transfer(1, 0x81, b"query", &mut input, 1, 10)
                    .unwrap()
                    .bytes,
                2
            );
            assert_eq!(&input[..2], b"ok");
            assert_eq!(host.quiesce(), Ok(()));
            assert_eq!(host.enumerate(&mut devices), Err(ServiceError::NotReady));
            assert_eq!(host.recover(), Ok(()));
        }
    }
}

#[cfg(feature = "display")]
pub mod display {
    //! Compatibility namespace for the single canonical bounded display
    //! contract. New code may import `nobro_display` directly; this feature does
    //! not define another lifecycle, capability, or receipt model.
    pub use nobro_display::*;
}

#[cfg(all(test, feature = "display"))]
mod canonical_display_tests {
    #[test]
    fn services_namespace_reexports_the_canonical_types() {
        let format: crate::display::PixelFormat = nobro_display::PixelFormat::Rgb565;
        let state: crate::display::DisplayState = nobro_display::DisplayState::Ready;
        assert_eq!(format, nobro_display::PixelFormat::Rgb565);
        assert_eq!(state, nobro_display::DisplayState::Ready);
    }
}

#[cfg(feature = "shell")]
pub mod shell {
    use super::ServiceError;

    #[derive(Clone, Copy)]
    pub struct ParsedCommand<const LINE_BYTES: usize, const ARGUMENTS: usize> {
        line: [u8; LINE_BYTES],
        line_len: usize,
        starts: [u16; ARGUMENTS],
        lengths: [u16; ARGUMENTS],
        arguments: usize,
    }

    impl<const LINE_BYTES: usize, const ARGUMENTS: usize> ParsedCommand<LINE_BYTES, ARGUMENTS> {
        pub const fn len(&self) -> usize {
            self.arguments
        }

        pub const fn is_empty(&self) -> bool {
            self.arguments == 0
        }

        pub fn arg(&self, index: usize) -> Option<&[u8]> {
            if index >= self.arguments {
                return None;
            }
            let start = usize::from(self.starts[index]);
            let len = usize::from(self.lengths[index]);
            Some(&self.line[start..start + len])
        }

        pub fn line(&self) -> &[u8] {
            &self.line[..self.line_len]
        }
    }

    pub struct BoundedShell<const LINE_BYTES: usize, const ARGUMENTS: usize>;

    impl<const LINE_BYTES: usize, const ARGUMENTS: usize> BoundedShell<LINE_BYTES, ARGUMENTS> {
        pub fn parse(input: &[u8]) -> Result<ParsedCommand<LINE_BYTES, ARGUMENTS>, ServiceError> {
            if input.is_empty()
                || input.len() > LINE_BYTES
                || LINE_BYTES > usize::from(u16::MAX)
                || ARGUMENTS == 0
                || input
                    .iter()
                    .any(|byte| !matches!(*byte, b' '..=b'~' | b'\t'))
            {
                return Err(ServiceError::InvalidConfig);
            }
            let mut command = ParsedCommand {
                line: [0; LINE_BYTES],
                line_len: input.len(),
                starts: [0; ARGUMENTS],
                lengths: [0; ARGUMENTS],
                arguments: 0,
            };
            command.line[..input.len()].copy_from_slice(input);
            let mut cursor = 0;
            while cursor < input.len() {
                while cursor < input.len() && matches!(input[cursor], b' ' | b'\t') {
                    cursor += 1;
                }
                if cursor == input.len() {
                    break;
                }
                if command.arguments == ARGUMENTS {
                    return Err(ServiceError::Full);
                }
                let start = cursor;
                while cursor < input.len() && !matches!(input[cursor], b' ' | b'\t') {
                    cursor += 1;
                }
                command.starts[command.arguments] = start as u16;
                command.lengths[command.arguments] = (cursor - start) as u16;
                command.arguments += 1;
            }
            if command.arguments == 0 {
                return Err(ServiceError::InvalidConfig);
            }
            Ok(command)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parser_uses_fixed_storage_and_fails_closed_on_overflow() {
            type Shell = BoundedShell<24, 3>;
            let command = Shell::parse(b"set motor 42").unwrap();
            assert_eq!(command.line(), b"set motor 42");
            assert_eq!(command.len(), 3);
            assert_eq!(command.arg(0), Some(b"set".as_slice()));
            assert_eq!(command.arg(2), Some(b"42".as_slice()));
            assert!(command.arg(3).is_none());
            assert!(matches!(
                Shell::parse(b"one two three four"),
                Err(ServiceError::Full)
            ));
            assert!(matches!(
                Shell::parse(b"bad\nline"),
                Err(ServiceError::InvalidConfig)
            ));
        }
    }
}
