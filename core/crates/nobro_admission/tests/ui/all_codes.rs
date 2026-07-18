use nobro_admission::{panic_admission, AdmissionErrorCode};

const _: () = panic_admission(AdmissionErrorCode::EmptyWorkload);
const _: () = panic_admission(AdmissionErrorCode::TooManyTasks);
const _: () = panic_admission(AdmissionErrorCode::DuplicateId);
const _: () = panic_admission(AdmissionErrorCode::InvalidDeadline);
const _: () = panic_admission(AdmissionErrorCode::InvalidJitter);
const _: () = panic_admission(AdmissionErrorCode::InvalidExecution);
const _: () = panic_admission(AdmissionErrorCode::InvalidBlocking);
const _: () = panic_admission(AdmissionErrorCode::UtilizationExceeded);
const _: () = panic_admission(AdmissionErrorCode::ResponseTimeExceeded);
const _: () = panic_admission(AdmissionErrorCode::FlashExceeded);
const _: () = panic_admission(AdmissionErrorCode::RamExceeded);
const _: () = panic_admission(AdmissionErrorCode::PoolExceeded);
const _: () = panic_admission(AdmissionErrorCode::ArithmeticOverflow);
const _: () = panic_admission(AdmissionErrorCode::WakeLatencyExceeded);
const _: () = panic_admission(AdmissionErrorCode::InvalidPhase);
const _: () = panic_admission(AdmissionErrorCode::InvalidInterruptPriority);
const _: () = panic_admission(AdmissionErrorCode::ReservedInterruptPriority);
const _: () = panic_admission(AdmissionErrorCode::InvalidInterruptContract);
const _: () = panic_admission(AdmissionErrorCode::UnsafeInterruptOperation);
const _: () = panic_admission(AdmissionErrorCode::InterruptStackExceeded);
const _: () = panic_admission(AdmissionErrorCode::InterruptResponseExceeded);

fn main() {}
