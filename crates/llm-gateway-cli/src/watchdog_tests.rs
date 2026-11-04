//! The watchdog's limits over the emulated control plane (story:pod-watchdog). A test module
//! of `watchdog.rs`, kept in its own file so that the shipped module's source names no
//! emulator. Nothing here reaches Runpod or allocates a pod.

use super::{Polled, Reason, Watch, owned, records};
use crate::heartbeat::Heartbeat;
use llm_runpod::{EmulatedRunpod, TAG_OWNER, TerminateAnswer};
use std::collections::BTreeMap;

const NOW: u64 = 10_000_000_000;
const MINUTE: u64 = 60_000;
const HOUR: u64 = 60 * MINUTE;

fn document(idle_minutes: u64, watchdog: &str) -> String {
    format!(
        "listen = \"127.0.0.1:0\"\ningress_auth = \"none\"\n\n[providers.runpod]\nkind = \"runpod-vllm\"\n\n\
         [models.small]\nprovider = \"runpod\"\nwires = [\"chat\"]\ncontext_window = 65536\n\
         hf_model = \"example/small-model\"\nimage = \"vllm/vllm-openai:v0.27.1\"\n\
         gpu_types = [\"NVIDIA L40S\"]\nmax_model_len = 1024\nidle_timeout_minutes = {idle_minutes}\n\n\
         [watchdog]\n{watchdog}"
    )
}

fn watch(idle_minutes: u64) -> Watch {
    let deployment = crate::parse(&document(idle_minutes, "")).unwrap();
    let config = deployment.watchdog.clone().unwrap();
    Watch::new(&deployment, &config)
}

fn tagged(owner: Option<&str>) -> BTreeMap<String, String> {
    owner
        .map(|owner| BTreeMap::from([(TAG_OWNER.to_owned(), owner.to_owned())]))
        .unwrap_or_default()
}

/// The gateway's own pod of `small`, started `age` ago.
fn own_pod(runpod: &EmulatedRunpod, age: u64) -> String {
    let id = runpod.insert_pod("b10x-llm-small", tagged(Some(crate::CONTROLLER)));
    runpod.set_started_at(&id, Some(NOW - age));
    id
}

/// Three pods that are not the gateway's, each started `age` ago: another owner's tag, no tag,
/// and the gateway's tag outside its name space.
fn foreign_pods(runpod: &EmulatedRunpod, age: u64) -> Vec<String> {
    let ids = vec![
        runpod.insert_pod("b10x-llm-small", tagged(Some("another-gateway"))),
        runpod.insert_pod("b10x-llm-small", tagged(None)),
        runpod.insert_pod("someone-elses-pod", tagged(Some(crate::CONTROLLER))),
    ];
    for id in &ids {
        runpod.set_started_at(id, Some(NOW - age));
    }
    ids
}

fn beat(age: u64, activity: Vec<(&str, Option<u64>, u64)>) -> Heartbeat {
    Heartbeat::now(
        NOW - age,
        crate::CONTROLLER,
        activity
            .into_iter()
            .map(|(route, last, in_flight)| (route.to_owned(), last, in_flight))
            .collect(),
    )
}

fn reasons(polled: &Polled) -> Vec<(String, Reason)> {
    match polled {
        Polled::Read(terminations) => terminations
            .iter()
            .map(|termination| {
                assert_eq!(termination.answer, TerminateAnswer::Terminated);
                (termination.pod.clone(), termination.reason)
            })
            .collect(),
        Polled::AccountUnreadable => panic!("the account was read"),
    }
}

/// A heartbeat a second old whose route had a call end a minute ago.
fn fresh() -> Heartbeat {
    beat(1_000, vec![("small", Some(NOW - MINUTE), 0)])
}

#[test]
fn a_balance_under_the_floor_terminates_an_owned_pod() {
    let mut runpod = EmulatedRunpod::new();
    let id = own_pod(&runpod, MINUTE);
    runpod.set_account(Some(4.99), Some(1.0));
    let polled = watch(30).poll(&mut runpod, Some(&fresh()), NOW);
    assert_eq!(reasons(&polled), vec![(id.clone(), Reason::MinBalance)]);
    assert_eq!(runpod.terminations(), vec![id]);
}

#[test]
fn a_stale_or_missing_heartbeat_terminates_an_owned_pod() {
    for heartbeat in [None, Some(beat(5 * MINUTE + 1, vec![("small", None, 1)]))] {
        let mut runpod = EmulatedRunpod::new();
        let id = own_pod(&runpod, MINUTE);
        let polled = watch(30).poll(&mut runpod, heartbeat.as_ref(), NOW);
        assert_eq!(reasons(&polled), vec![(id, Reason::StaleHeartbeat)]);
    }
    let mut runpod = EmulatedRunpod::new();
    own_pod(&runpod, MINUTE);
    let at_the_limit = beat(5 * MINUTE, vec![("small", None, 1)]);
    let polled = watch(30).poll(&mut runpod, Some(&at_the_limit), NOW);
    assert_eq!(
        reasons(&polled),
        vec![],
        "exactly stale_minutes old is not stale yet"
    );
}

#[test]
fn a_pod_past_its_maximum_runtime_is_terminated_even_while_busy() {
    let mut runpod = EmulatedRunpod::new();
    let id = own_pod(&runpod, 24 * HOUR + 1);
    let busy = beat(1_000, vec![("small", Some(NOW - MINUTE), 3)]);
    let polled = watch(30).poll(&mut runpod, Some(&busy), NOW);
    assert_eq!(reasons(&polled), vec![(id, Reason::MaxRuntime)]);
}

#[test]
fn a_pod_not_ready_past_the_window_is_terminated_and_a_ready_one_is_kept() {
    let mut runpod = EmulatedRunpod::new();
    let id = own_pod(&runpod, 46 * MINUTE);
    let mut watch = watch(30);
    let polled = watch.poll(&mut runpod, Some(&fresh()), NOW);
    assert_eq!(reasons(&polled), vec![], "a pod that answers ready is kept");
    runpod.probe_unreachable(true);
    let polled = watch.poll(&mut runpod, Some(&fresh()), NOW + MINUTE);
    assert_eq!(
        reasons(&polled),
        vec![],
        "ready a minute ago: the window starts again"
    );
    let later = Heartbeat::now(
        NOW + 46 * MINUTE - 1_000,
        crate::CONTROLLER,
        vec![("small".to_owned(), None, 1)],
    );
    let polled = watch.poll(&mut runpod, Some(&later), NOW + 46 * MINUTE);
    assert_eq!(reasons(&polled), vec![(id, Reason::NotReady)]);
}

#[test]
fn an_idle_pod_is_terminated_and_one_with_a_call_in_flight_is_kept() {
    let mut runpod = EmulatedRunpod::new();
    let id = own_pod(&runpod, 2 * HOUR);
    let mut watch = watch(30);
    let busy = beat(1_000, vec![("small", Some(NOW - HOUR), 1)]);
    assert_eq!(reasons(&watch.poll(&mut runpod, Some(&busy), NOW)), vec![]);
    let recent = beat(1_000, vec![("small", Some(NOW - 29 * MINUTE), 0)]);
    assert_eq!(
        reasons(&watch.poll(&mut runpod, Some(&recent), NOW)),
        vec![]
    );
    let unknown = beat(1_000, vec![]);
    assert_eq!(
        reasons(&watch.poll(&mut runpod, Some(&unknown), NOW)),
        vec![],
        "a route the heartbeat does not list has no idle limit"
    );
    let idle = beat(1_000, vec![("small", Some(NOW - 31 * MINUTE), 0)]);
    assert_eq!(
        reasons(&watch.poll(&mut runpod, Some(&idle), NOW)),
        vec![(id, Reason::Idle)]
    );
}

#[test]
fn the_idle_window_is_never_shorter_than_the_stale_window() {
    let mut runpod = EmulatedRunpod::new();
    let id = own_pod(&runpod, 2 * HOUR);
    let mut watch = watch(0);
    let two = beat(1_000, vec![("small", Some(NOW - 2 * MINUTE), 0)]);
    assert_eq!(reasons(&watch.poll(&mut runpod, Some(&two), NOW)), vec![]);
    let six = beat(1_000, vec![("small", Some(NOW - 6 * MINUTE), 0)]);
    assert_eq!(
        reasons(&watch.poll(&mut runpod, Some(&six), NOW)),
        vec![(id, Reason::Idle)]
    );
}

#[test]
fn a_fresh_heartbeat_with_activity_keeps_the_pod() {
    let mut runpod = EmulatedRunpod::new();
    let id = own_pod(&runpod, HOUR);
    runpod.set_account(Some(50.0), Some(1.0));
    let polled = watch(30).poll(&mut runpod, Some(&fresh()), NOW);
    assert_eq!(reasons(&polled), vec![]);
    assert!(runpod.terminations().is_empty());
    assert!(runpod.pods().iter().any(|pod| pod.id == id));
}

#[test]
fn a_pod_that_is_not_the_gateways_is_never_terminated_whatever_holds() {
    let mut runpod = EmulatedRunpod::new();
    let foreign = foreign_pods(&runpod, 48 * HOUR);
    runpod.set_account(Some(0.0), Some(9.0));
    runpod.probe_unreachable(true);
    let mut watch = watch(0);
    for at in [NOW, NOW + 2 * HOUR] {
        let polled = watch.poll(&mut runpod, None, at);
        assert_eq!(reasons(&polled), vec![]);
    }
    assert!(runpod.terminations().is_empty());
    let summaries = llm_runpod::RunpodAccount::account(&mut runpod).unwrap();
    assert_eq!(
        summaries.pods.iter().filter(|pod| owned(pod)).count(),
        0,
        "{foreign:?}"
    );
    let id = own_pod(&runpod, MINUTE);
    let polled = watch.poll(&mut runpod, None, NOW);
    assert_eq!(reasons(&polled), vec![(id.clone(), Reason::MinBalance)]);
    assert_eq!(
        runpod.terminations(),
        vec![id],
        "only the gateway's own pod"
    );
}

#[test]
fn an_unreadable_account_terminates_nothing_and_is_recorded() {
    let mut runpod = EmulatedRunpod::new();
    own_pod(&runpod, 48 * HOUR);
    runpod.account_unanswered(true);
    let polled = watch(30).poll(&mut runpod, None, NOW);
    assert_eq!(polled, Polled::AccountUnreadable);
    assert!(runpod.terminations().is_empty());
    let lines = records(&polled, NOW);
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0].contains("\"event\":\"account-unreadable\""),
        "{}",
        lines[0]
    );
}

#[test]
fn each_termination_is_one_json_line_naming_the_pod_and_the_reason() {
    let mut runpod = EmulatedRunpod::new();
    let id = own_pod(&runpod, MINUTE);
    let polled = watch(30).poll(&mut runpod, None, NOW);
    let lines = records(&polled, NOW);
    assert_eq!(lines.len(), 1);
    let record: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(record["format"], super::RECORD_FORMAT);
    assert_eq!(record["pod"], id.as_str());
    assert_eq!(record["reason"], "stale-heartbeat");
    assert_eq!(record["answer"], "terminated");
    assert_eq!(record["time_ms"], NOW);
}

#[test]
fn the_watchdog_table_has_its_defaults_and_refuses_values_outside_its_ranges() {
    let deployment = crate::parse(&document(30, "")).unwrap();
    let config = deployment.watchdog.unwrap();
    assert_eq!(
        (
            config.heartbeat_file.clone(),
            config.heartbeat_seconds,
            config.poll_seconds,
            config.stale_minutes,
            config.not_ready_minutes,
            config.max_runtime_hours,
        ),
        (None, 10, 60, 5, 45, 24)
    );
    assert!((config.min_balance_usd - 5.0).abs() < f64::EPSILON);
    let without = crate::parse(&document(30, "").replace("\n[watchdog]\n", "\n")).unwrap();
    assert!(without.watchdog.is_none());
    for (line, key) in [
        ("heartbeat_seconds = 0\n", "watchdog.heartbeat_seconds"),
        ("heartbeat_seconds = 61\n", "watchdog.heartbeat_seconds"),
        ("poll_seconds = 9\n", "watchdog.poll_seconds"),
        ("stale_minutes = 0\n", "watchdog.stale_minutes"),
        (
            "stale_minutes = 1\nheartbeat_seconds = 30\n",
            "watchdog.stale_minutes",
        ),
        ("not_ready_minutes = 0\n", "watchdog.not_ready_minutes"),
        ("max_runtime_hours = 721\n", "watchdog.max_runtime_hours"),
        ("min_balance_usd = -1.0\n", "watchdog.min_balance_usd"),
        ("min_balance_usd = nan\n", "watchdog.min_balance_usd"),
        ("heartbeat_file = \"\"\n", "watchdog.heartbeat_file"),
    ] {
        let refusal = crate::parse(&document(30, line)).unwrap_err();
        assert_eq!(refusal.code(), "config:value", "{line}");
        assert!(refusal.to_string().contains(key), "{line}: {refusal}");
    }
    let unknown = crate::parse(&document(30, "terminate_everything = true\n")).unwrap_err();
    assert_eq!(unknown.code(), "config:schema");
}
