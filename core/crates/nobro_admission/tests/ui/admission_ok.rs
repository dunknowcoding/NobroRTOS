use nobro_admission::{
    nobro_admit, AdmissionProfile, AdmittedWorkload, TaskContract,
};

const PROFILE: AdmissionProfile = AdmissionProfile::new(4096, 1024, 4, 1);
const TASK: TaskContract =
    TaskContract::new(1).deadline(10_000, 10_000, 0, 100, 0).memory(512, 128, 1);
const ADMITTED: AdmittedWorkload<1> = nobro_admit!([TASK], PROFILE);

fn main() {
    assert_eq!(ADMITTED.task_count, 1);
}
