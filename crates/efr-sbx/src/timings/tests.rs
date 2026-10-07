use super::Timings;

#[test]
fn each_lap_is_one_step_in_order() {
    let mut timings = Timings::start();
    timings.lap("plan");
    timings.lap("launch");
    let phases: Vec<String> = timings.into_steps().into_iter().map(|step| step.phase).collect();
    assert_eq!(phases, ["plan", "launch"]);
}
