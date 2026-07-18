use nobro_admission::{
    nobro_admit, AdmissionProfile, AdmittedWorkload, TaskContract,
};

const PROFILE: AdmissionProfile = AdmissionProfile::new(4096, 1024, 4, 1);
const BAD_PERIOD: TaskContract =
    TaskContract::new(1).deadline(0, 0, 0, 100, 0).memory(512, 128, 1);
const _: AdmittedWorkload<1> = nobro_admit!([BAD_PERIOD], PROFILE);

fn main() {}
