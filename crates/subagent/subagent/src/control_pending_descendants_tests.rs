use super::*;
use std::time::Duration;

#[derive(Default)]
struct Resident {
    id: &'static str,
    lineage: HashSet<usize>,
}

#[derive(Default)]
struct Fixtures {
    materializations: parking_lot::Mutex<HashMap<u64, HashSet<usize>>>,
    activations: parking_lot::Mutex<HashMap<String, Arc<parking_lot::Mutex<Resident>>>>,
}

impl Fixtures {
    fn read(&self) -> Option<bool> {
        try_pending_descendants(7, &self.materializations, &self.activations, |activation| {
            activation.id != "parent" && activation.lineage.contains(&7)
        })
    }
}

fn assert_nonblocking(fixtures: Arc<Fixtures>, release: impl FnOnce()) {
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || sender.send(fixtures.read()).unwrap());
    let result = receiver.recv_timeout(Duration::from_millis(250));
    // Release even on failure, so a blocking implementation fails instead of
    // stranding the test worker while the guard remains held.
    release();
    reader.join().unwrap();
    assert_eq!(
        result.unwrap(),
        None,
        "contention must be reported as unknown without waiting"
    );
}

#[test]
fn control_pending_descendants_never_waits_on_registry_or_materialization_locks() {
    let fixtures = Arc::new(Fixtures::default());
    let guard = fixtures.materializations.lock();
    assert_nonblocking(fixtures.clone(), || drop(guard));
    let guard = fixtures.activations.lock();
    assert_nonblocking(fixtures.clone(), || drop(guard));
}

#[test]
fn control_pending_descendants_never_waits_on_an_activation_lock() {
    let fixtures = Arc::new(Fixtures::default());
    let child = Arc::new(parking_lot::Mutex::new(Resident {
        id: "child",
        lineage: HashSet::from([7]),
    }));
    fixtures
        .activations
        .lock()
        .insert("child".into(), child.clone());
    let guard = child.lock();
    assert_nonblocking(fixtures.clone(), || drop(guard));
    assert_eq!(fixtures.read(), Some(true));
}

#[test]
fn control_pending_descendants_covers_materialization_and_idle_settlement_gaps() {
    let fixtures = Fixtures::default();
    assert_eq!(fixtures.read(), Some(false));
    fixtures
        .materializations
        .lock()
        .insert(1, HashSet::from([8]));
    assert_eq!(fixtures.read(), Some(false));
    fixtures
        .materializations
        .lock()
        .insert(2, HashSet::from([7]));
    assert_eq!(fixtures.read(), Some(true));
    fixtures.materializations.lock().clear();
    fixtures.activations.lock().insert(
        "parent".into(),
        Arc::new(parking_lot::Mutex::new(Resident {
            id: "parent",
            lineage: HashSet::from([7]),
        })),
    );
    fixtures.activations.lock().insert(
        "unrelated".into(),
        Arc::new(parking_lot::Mutex::new(Resident {
            id: "unrelated",
            lineage: HashSet::from([8]),
        })),
    );
    assert_eq!(fixtures.read(), Some(false));
    fixtures.activations.lock().insert(
        "child".into(),
        Arc::new(parking_lot::Mutex::new(Resident {
            id: "child",
            lineage: HashSet::from([7]),
        })),
    );
    assert_eq!(
        fixtures.read(),
        Some(true),
        "a resident child remains pending until its late settlement is delivered"
    );
    fixtures.activations.lock().remove("child");
    assert_eq!(fixtures.read(), Some(false));
}
