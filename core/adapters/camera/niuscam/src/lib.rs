//! Allocation-free ownership bridge for NiusCam-class transports.
//!
//! The Arduino facade and a native transport can implement the same small
//! transport contract. Sensor pin maps and vendor frame buffers stay in
//! NiusCam; Nobro owns logical-instance selection, bounds and lifecycle.
#![cfg_attr(not(test), no_std)]

use nobro_camera::{
    CameraBackend, CameraBackendIdentity, CameraFrame, CameraState, MountableCameraBackend,
};

pub const BACKEND_ID: &str = "niuscam-arduino";

pub trait NiusCamTransport {
    type Frame<'a>: CameraFrame
    where
        Self: 'a;

    fn state(&self) -> CameraState;
    fn begin(&mut self) -> bool;
    fn capture(&mut self) -> Option<Self::Frame<'_>>;
    fn quiesce(&mut self) -> bool;
    fn recover(&mut self) -> bool;
}

pub struct NiusCamBackend<T> {
    transport: T,
    sensor_id: &'static str,
    max_frame_bytes: u32,
    max_in_flight: u8,
}

impl<T> NiusCamBackend<T> {
    pub const fn new(
        transport: T,
        sensor_id: &'static str,
        max_frame_bytes: u32,
        max_in_flight: u8,
    ) -> Self {
        Self {
            transport,
            sensor_id,
            max_frame_bytes,
            max_in_flight,
        }
    }

    pub const fn transport(&self) -> &T {
        &self.transport
    }

    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }
}

impl<T: NiusCamTransport> CameraBackend for NiusCamBackend<T> {
    type Frame<'a>
        = T::Frame<'a>
    where
        Self: 'a;

    fn state(&self) -> CameraState {
        self.transport.state()
    }

    fn capture(&mut self) -> Option<Self::Frame<'_>> {
        self.transport.capture()
    }

    fn recover(&mut self) -> bool {
        self.transport.recover()
    }
}

impl<T: NiusCamTransport> MountableCameraBackend for NiusCamBackend<T> {
    fn identity(&self) -> CameraBackendIdentity {
        CameraBackendIdentity {
            backend_id: BACKEND_ID,
            sensor_id: self.sensor_id,
            max_frame_bytes: self.max_frame_bytes,
            max_in_flight: self.max_in_flight,
        }
    }

    fn mount_camera(&mut self) -> bool {
        self.transport.begin()
    }

    fn quiesce(&mut self) -> bool {
        self.transport.quiesce()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nobro_camera::{
        CameraInstanceId, CaptureContract, FrameMetadata, MountedCamera, PixelFormat, StreamBudget,
    };

    struct Frame([u8; 8]);

    impl CameraFrame for Frame {
        fn metadata(&self) -> FrameMetadata {
            FrameMetadata {
                width: 2,
                height: 2,
                bytes: self.0.len() as u32,
                timestamp_us: 10,
                format: PixelFormat::Grayscale,
            }
        }

        fn data(&self) -> &[u8] {
            &self.0
        }
    }

    struct Transport {
        state: CameraState,
    }

    impl NiusCamTransport for Transport {
        type Frame<'a> = Frame;

        fn state(&self) -> CameraState {
            self.state
        }

        fn begin(&mut self) -> bool {
            self.state = CameraState::Ready;
            true
        }

        fn capture(&mut self) -> Option<Self::Frame<'_>> {
            (self.state == CameraState::Ready).then_some(Frame([7; 8]))
        }

        fn quiesce(&mut self) -> bool {
            self.state = CameraState::Suspended;
            true
        }

        fn recover(&mut self) -> bool {
            self.state = CameraState::Ready;
            true
        }
    }

    #[test]
    fn selected_backend_mounts_once_and_preserves_bounds() {
        let backend = NiusCamBackend::new(
            Transport {
                state: CameraState::Down,
            },
            "ov3660",
            8,
            1,
        );
        let (mut camera, receipt) = MountedCamera::mount(
            CameraInstanceId::new(3),
            backend,
            StreamBudget::new(2, 16, 1),
        )
        .unwrap_or_else(|_| panic!("valid NiusCam mount rejected"));
        assert_eq!(receipt.instance.get(), 3);
        assert_eq!(receipt.backend.backend_id, BACKEND_ID);
        let frame = camera.capture_at(1, CaptureContract::by(2, 8)).unwrap();
        assert_eq!(frame.data(), &[7; 8]);
        drop(frame);
        camera.quiesce().unwrap();
        assert_eq!(camera.recover().unwrap().generation, 2);
    }
}
