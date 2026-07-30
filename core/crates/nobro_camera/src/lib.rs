//! Allocation-free camera contracts with explicit deadline and memory admission.
#![cfg_attr(not(test), no_std)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Jpeg,
    Grayscale,
    Rgb565,
    Yuv422,
    Rgb888,
    Other(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraState {
    Down,
    Starting,
    Ready,
    Suspended,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CameraInstanceId(u16);

impl CameraInstanceId {
    pub const PRIMARY: Self = Self(0);

    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CameraBackendIdentity {
    pub backend_id: &'static str,
    pub sensor_id: &'static str,
    pub max_frame_bytes: u32,
    pub max_in_flight: u8,
}

impl CameraBackendIdentity {
    pub const fn is_valid(self) -> bool {
        !self.backend_id.is_empty()
            && !self.sensor_id.is_empty()
            && self.max_frame_bytes != 0
            && self.max_in_flight != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CameraMountReceipt {
    pub instance: CameraInstanceId,
    pub backend: CameraBackendIdentity,
    pub generation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameMetadata {
    pub width: u16,
    pub height: u16,
    pub bytes: u32,
    pub timestamp_us: u64,
    pub format: PixelFormat,
}

/// A frame lease. Dropping an implementation must return its DMA buffer to the backend.
pub trait CameraFrame {
    fn metadata(&self) -> FrameMetadata;
    fn data(&self) -> &[u8];
}

/// One independently selectable camera implementation.
pub trait CameraBackend {
    type Frame<'a>: CameraFrame
    where
        Self: 'a;

    fn state(&self) -> CameraState;
    fn capture(&mut self) -> Option<Self::Frame<'_>>;
    fn recover(&mut self) -> bool;
}

/// Lifecycle needed to mount a camera as one owned logical stack instance.
pub trait MountableCameraBackend: CameraBackend {
    fn identity(&self) -> CameraBackendIdentity;
    fn mount_camera(&mut self) -> bool;
    fn quiesce(&mut self) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureContract {
    pub deadline_us: u64,
    pub max_frame_bytes: u32,
    pub max_processing_us: u32,
}

impl CaptureContract {
    pub const fn by(deadline_us: u64, max_frame_bytes: u32) -> Self {
        Self {
            deadline_us,
            max_frame_bytes,
            max_processing_us: 0,
        }
    }

    pub const fn processing_budget(mut self, max_processing_us: u32) -> Self {
        self.max_processing_us = max_processing_us;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamBudget {
    pub max_frames_per_window: u16,
    pub max_bytes_per_window: u32,
    pub max_in_flight: u8,
}

impl StreamBudget {
    pub const fn new(frames: u16, bytes: u32, in_flight: u8) -> Self {
        Self {
            max_frames_per_window: frames,
            max_bytes_per_window: bytes,
            max_in_flight: in_flight,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CameraDiagnostics {
    pub frames_accepted: u32,
    pub frames_dropped: u32,
    pub bytes_accepted: u64,
    pub deadline_rejections: u32,
    pub memory_rejections: u32,
    pub backpressure_rejections: u32,
    pub capture_failures: u32,
    pub recoveries: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraError {
    InvalidConfig,
    NotReady,
    DeadlineElapsed,
    WindowExhausted,
    Backpressured,
    CaptureFailed,
    FrameTooLarge,
}

/// An admitted frame whose drop automatically releases pipeline backpressure.
pub struct AdmittedFrame<'a, F: CameraFrame> {
    frame: F,
    in_flight: &'a mut u8,
}

impl<F: CameraFrame> CameraFrame for AdmittedFrame<'_, F> {
    fn metadata(&self) -> FrameMetadata {
        self.frame.metadata()
    }

    fn data(&self) -> &[u8] {
        self.frame.data()
    }
}

impl<F: CameraFrame> Drop for AdmittedFrame<'_, F> {
    fn drop(&mut self) {
        *self.in_flight = self.in_flight.saturating_sub(1);
    }
}

/// Admission and accounting shared by camera, storage, AI, and transport consumers.
pub struct CameraPipeline<B> {
    backend: B,
    budget: StreamBudget,
    frames_in_window: u16,
    bytes_in_window: u32,
    in_flight: u8,
    diagnostics: CameraDiagnostics,
}

impl<B: CameraBackend> CameraPipeline<B> {
    pub const fn new(backend: B, budget: StreamBudget) -> Self {
        Self {
            backend,
            budget,
            frames_in_window: 0,
            bytes_in_window: 0,
            in_flight: 0,
            diagnostics: CameraDiagnostics {
                frames_accepted: 0,
                frames_dropped: 0,
                bytes_accepted: 0,
                deadline_rejections: 0,
                memory_rejections: 0,
                backpressure_rejections: 0,
                capture_failures: 0,
                recoveries: 0,
            },
        }
    }

    pub fn capture_at(
        &mut self,
        now_us: u64,
        contract: CaptureContract,
    ) -> Result<AdmittedFrame<'_, B::Frame<'_>>, CameraError> {
        if self.backend.state() != CameraState::Ready {
            self.reject();
            return Err(CameraError::NotReady);
        }
        if now_us > contract.deadline_us {
            self.diagnostics.deadline_rejections =
                self.diagnostics.deadline_rejections.saturating_add(1);
            self.reject();
            return Err(CameraError::DeadlineElapsed);
        }
        if self.frames_in_window >= self.budget.max_frames_per_window {
            self.diagnostics.memory_rejections =
                self.diagnostics.memory_rejections.saturating_add(1);
            self.reject();
            return Err(CameraError::WindowExhausted);
        }
        if self.in_flight >= self.budget.max_in_flight {
            self.diagnostics.backpressure_rejections =
                self.diagnostics.backpressure_rejections.saturating_add(1);
            self.reject();
            return Err(CameraError::Backpressured);
        }
        let frame = match self.backend.capture() {
            Some(frame) => frame,
            None => {
                self.diagnostics.capture_failures =
                    self.diagnostics.capture_failures.saturating_add(1);
                self.diagnostics.frames_dropped = self.diagnostics.frames_dropped.saturating_add(1);
                return Err(CameraError::CaptureFailed);
            }
        };
        let bytes = frame.metadata().bytes;
        if bytes > contract.max_frame_bytes
            || self.bytes_in_window.saturating_add(bytes) > self.budget.max_bytes_per_window
        {
            self.diagnostics.memory_rejections =
                self.diagnostics.memory_rejections.saturating_add(1);
            self.diagnostics.frames_dropped = self.diagnostics.frames_dropped.saturating_add(1);
            return Err(CameraError::FrameTooLarge);
        }
        self.frames_in_window = self.frames_in_window.saturating_add(1);
        self.bytes_in_window = self.bytes_in_window.saturating_add(bytes);
        self.in_flight = self.in_flight.saturating_add(1);
        self.diagnostics.frames_accepted = self.diagnostics.frames_accepted.saturating_add(1);
        self.diagnostics.bytes_accepted =
            self.diagnostics.bytes_accepted.saturating_add(bytes as u64);
        Ok(AdmittedFrame {
            frame,
            in_flight: &mut self.in_flight,
        })
    }

    pub fn reset_window(&mut self) {
        self.frames_in_window = 0;
        self.bytes_in_window = 0;
    }

    pub fn recover(&mut self) -> bool {
        let recovered = self.backend.recover();
        if recovered {
            self.diagnostics.recoveries = self.diagnostics.recoveries.saturating_add(1);
        }
        recovered
    }

    pub const fn diagnostics(&self) -> CameraDiagnostics {
        self.diagnostics
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn into_backend(self) -> B {
        self.backend
    }

    fn reject(&mut self) {
        self.diagnostics.frames_dropped = self.diagnostics.frames_dropped.saturating_add(1);
    }
}

pub struct CameraMountError<B> {
    pub backend: B,
    pub error: CameraError,
}

/// One camera backend consumed by one logical instance.
pub struct MountedCamera<B> {
    pipeline: CameraPipeline<B>,
    receipt: CameraMountReceipt,
}

impl<B: MountableCameraBackend> MountedCamera<B> {
    pub fn mount(
        instance: CameraInstanceId,
        mut backend: B,
        budget: StreamBudget,
    ) -> Result<(Self, CameraMountReceipt), CameraMountError<B>> {
        let identity = backend.identity();
        if !identity.is_valid()
            || budget.max_frames_per_window == 0
            || budget.max_bytes_per_window < identity.max_frame_bytes
            || budget.max_in_flight == 0
            || budget.max_in_flight > identity.max_in_flight
        {
            return Err(CameraMountError {
                backend,
                error: CameraError::InvalidConfig,
            });
        }
        if !backend.mount_camera() || backend.state() != CameraState::Ready {
            return Err(CameraMountError {
                backend,
                error: CameraError::NotReady,
            });
        }
        let receipt = CameraMountReceipt {
            instance,
            backend: identity,
            generation: 1,
        };
        Ok((
            Self {
                pipeline: CameraPipeline::new(backend, budget),
                receipt,
            },
            receipt,
        ))
    }

    pub const fn receipt(&self) -> CameraMountReceipt {
        self.receipt
    }

    pub fn capture_at(
        &mut self,
        now_us: u64,
        contract: CaptureContract,
    ) -> Result<AdmittedFrame<'_, B::Frame<'_>>, CameraError> {
        self.pipeline.capture_at(now_us, contract)
    }

    pub fn reset_window(&mut self) {
        self.pipeline.reset_window();
    }

    pub fn quiesce(&mut self) -> Result<(), CameraError> {
        self.pipeline
            .backend_mut()
            .quiesce()
            .then_some(())
            .ok_or(CameraError::CaptureFailed)
    }

    pub fn recover(&mut self) -> Result<CameraMountReceipt, CameraError> {
        if !self.pipeline.recover() {
            return Err(CameraError::CaptureFailed);
        }
        self.receipt.generation = self.receipt.generation.saturating_add(1);
        Ok(self.receipt)
    }

    pub fn backend(&self) -> &B {
        self.pipeline.backend()
    }

    pub fn into_backend(self) -> B {
        self.pipeline.into_backend()
    }
}

/// Cheap allocation-free AI feature useful for scheduling tests and tiny classifiers.
pub fn sampled_mean(bytes: &[u8], stride: usize) -> Option<u8> {
    if bytes.is_empty() || stride == 0 {
        return None;
    }
    let mut sum = 0u64;
    let mut count = 0u64;
    let mut index = 0;
    while index < bytes.len() {
        sum = sum.saturating_add(u64::from(bytes[index]));
        count += 1;
        index = index.saturating_add(stride);
    }
    Some((sum / count) as u8)
}

/// Whole-workflow limits shared by camera capture, AI, storage, and transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkflowBudget {
    pub max_frame_bytes: u64,
    pub max_ai_us: u64,
    pub max_storage_bytes: u64,
    pub max_transport_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkflowUsage {
    pub frames: u32,
    pub frame_bytes: u64,
    pub ai_us: u64,
    pub storage_bytes: u64,
    pub transport_bytes: u64,
    pub rejected: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkflowError {
    FrameMemory,
    AiCpu,
    Storage,
    Transport,
}

pub struct WorkflowAccountant {
    budget: WorkflowBudget,
    usage: WorkflowUsage,
}

impl WorkflowAccountant {
    pub const fn new(budget: WorkflowBudget) -> Self {
        Self {
            budget,
            usage: WorkflowUsage {
                frames: 0,
                frame_bytes: 0,
                ai_us: 0,
                storage_bytes: 0,
                transport_bytes: 0,
                rejected: 0,
            },
        }
    }

    pub fn admit(
        &mut self,
        frame_bytes: u32,
        ai_us: u32,
        storage_bytes: u32,
        transport_bytes: u32,
    ) -> Result<(), WorkflowError> {
        let next_frame = self
            .usage
            .frame_bytes
            .saturating_add(u64::from(frame_bytes));
        let next_ai = self.usage.ai_us.saturating_add(u64::from(ai_us));
        let next_storage = self
            .usage
            .storage_bytes
            .saturating_add(u64::from(storage_bytes));
        let next_transport = self
            .usage
            .transport_bytes
            .saturating_add(u64::from(transport_bytes));
        let error = if next_frame > self.budget.max_frame_bytes {
            Some(WorkflowError::FrameMemory)
        } else if next_ai > self.budget.max_ai_us {
            Some(WorkflowError::AiCpu)
        } else if next_storage > self.budget.max_storage_bytes {
            Some(WorkflowError::Storage)
        } else if next_transport > self.budget.max_transport_bytes {
            Some(WorkflowError::Transport)
        } else {
            None
        };
        if let Some(error) = error {
            self.usage.rejected = self.usage.rejected.saturating_add(1);
            return Err(error);
        }
        self.usage.frames = self.usage.frames.saturating_add(1);
        self.usage.frame_bytes = next_frame;
        self.usage.ai_us = next_ai;
        self.usage.storage_bytes = next_storage;
        self.usage.transport_bytes = next_transport;
        Ok(())
    }

    pub const fn usage(&self) -> WorkflowUsage {
        self.usage
    }

    pub fn reset_window(&mut self) {
        let rejected = self.usage.rejected;
        self.usage = WorkflowUsage {
            frames: 0,
            frame_bytes: 0,
            ai_us: 0,
            storage_bytes: 0,
            transport_bytes: 0,
            rejected,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Frame([u8; 16]);
    impl CameraFrame for Frame {
        fn metadata(&self) -> FrameMetadata {
            FrameMetadata {
                width: 4,
                height: 4,
                bytes: 16,
                timestamp_us: 10,
                format: PixelFormat::Grayscale,
            }
        }
        fn data(&self) -> &[u8] {
            &self.0
        }
    }

    struct Camera {
        state: CameraState,
        fail: bool,
    }
    impl CameraBackend for Camera {
        type Frame<'a> = Frame;
        fn state(&self) -> CameraState {
            self.state
        }
        fn capture(&mut self) -> Option<Self::Frame<'_>> {
            if self.fail {
                None
            } else {
                Some(Frame([8; 16]))
            }
        }
        fn recover(&mut self) -> bool {
            self.state = CameraState::Ready;
            true
        }
    }

    impl MountableCameraBackend for Camera {
        fn identity(&self) -> CameraBackendIdentity {
            CameraBackendIdentity {
                backend_id: "test-camera",
                sensor_id: "ov-test",
                max_frame_bytes: 16,
                max_in_flight: 1,
            }
        }

        fn mount_camera(&mut self) -> bool {
            self.state = CameraState::Ready;
            true
        }

        fn quiesce(&mut self) -> bool {
            self.state = CameraState::Suspended;
            true
        }
    }

    #[test]
    fn accounts_deadline_memory_and_backpressure() {
        let mut pipeline = CameraPipeline::new(
            Camera {
                state: CameraState::Ready,
                fail: false,
            },
            StreamBudget::new(2, 32, 1),
        );
        assert_eq!(
            pipeline.capture_at(11, CaptureContract::by(10, 16)).err(),
            Some(CameraError::DeadlineElapsed)
        );
        assert_eq!(
            pipeline.capture_at(1, CaptureContract::by(10, 8)).err(),
            Some(CameraError::FrameTooLarge)
        );
        let frame = pipeline.capture_at(1, CaptureContract::by(10, 16)).unwrap();
        assert_eq!(sampled_mean(frame.data(), 2), Some(8));
        drop(frame);
        assert!(pipeline.capture_at(1, CaptureContract::by(10, 16)).is_ok());

        let mut blocked = CameraPipeline::new(
            Camera {
                state: CameraState::Ready,
                fail: false,
            },
            StreamBudget::new(1, 16, 0),
        );
        assert_eq!(
            blocked.capture_at(1, CaptureContract::by(10, 16)).err(),
            Some(CameraError::Backpressured)
        );
    }

    #[test]
    fn recovery_is_explicit() {
        let mut pipeline = CameraPipeline::new(
            Camera {
                state: CameraState::Faulted,
                fail: false,
            },
            StreamBudget::new(1, 16, 1),
        );
        assert_eq!(
            pipeline.capture_at(1, CaptureContract::by(2, 16)).err(),
            Some(CameraError::NotReady)
        );
        assert!(pipeline.recover());
        assert!(pipeline.capture_at(1, CaptureContract::by(2, 16)).is_ok());
    }

    #[test]
    fn owned_camera_mount_binds_backend_and_recovery_generation() {
        let backend = Camera {
            state: CameraState::Down,
            fail: false,
        };
        let (mut mounted, receipt) = MountedCamera::mount(
            CameraInstanceId::new(2),
            backend,
            StreamBudget::new(2, 32, 1),
        )
        .unwrap_or_else(|_| panic!("valid camera mount rejected"));
        assert_eq!(receipt.instance.get(), 2);
        assert_eq!(receipt.backend.backend_id, "test-camera");
        assert!(mounted.capture_at(1, CaptureContract::by(10, 16)).is_ok());
        mounted.quiesce().unwrap();
        assert_eq!(mounted.backend().state(), CameraState::Suspended);
        assert_eq!(mounted.recover().unwrap().generation, 2);
    }

    #[test]
    fn camera_mount_rejects_budget_smaller_than_backend_contract() {
        let backend = Camera {
            state: CameraState::Down,
            fail: false,
        };
        let error = MountedCamera::mount(
            CameraInstanceId::PRIMARY,
            backend,
            StreamBudget::new(1, 8, 1),
        )
        .err()
        .unwrap_or_else(|| panic!("invalid camera mount accepted"));
        assert_eq!(error.error, CameraError::InvalidConfig);
        assert_eq!(error.backend.state(), CameraState::Down);
    }

    #[test]
    fn three_camera_ai_storage_transport_workflow_is_bounded() {
        let mut workflow = WorkflowAccountant::new(WorkflowBudget {
            max_frame_bytes: 420_000,
            max_ai_us: 30_000,
            max_storage_bytes: 300_000,
            max_transport_bytes: 180_000,
        });
        // Representative JPEG envelopes for OV2640, OV3660, and OV5640 nodes.
        assert!(workflow.admit(90_000, 7_000, 90_000, 45_000).is_ok());
        assert!(workflow.admit(120_000, 8_000, 120_000, 60_000).is_ok());
        assert!(workflow.admit(180_000, 12_000, 80_000, 70_000).is_ok());
        assert_eq!(workflow.usage().frames, 3);
        assert_eq!(
            workflow.admit(40_000, 1_000, 1_000, 1_000),
            Err(WorkflowError::FrameMemory)
        );
        assert_eq!(workflow.usage().rejected, 1);
        workflow.reset_window();
        assert_eq!(workflow.usage().frames, 0);
    }
}
