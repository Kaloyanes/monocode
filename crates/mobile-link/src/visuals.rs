//! Interactive visuals on the phone. The program runs here, in the same
//! sandboxed runtime the desktop uses (compiled from its source), so a slider
//! drag never waits on the network. The platform draws the component tree.
//!
//! A `VisualRuntime` is not `Send`, so every runtime lives on one worker
//! thread and callers talk to it over a channel.
use serde_json::{json, Value};
use std::sync::{mpsc, Mutex, OnceLock};

// The phone uses only the runtime, not the desktop's checking helpers.
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
#[path = "../../monocode-core/src"]
pub(crate) mod shared {
    pub mod visual;
}

// The desktop's 3D projection, so a scene orbits the same on both screens.
// Its `monocode_core::visual` paths resolve to `shared` through the crate root.
#[cfg(not(target_os = "android"))]
#[path = "../../monocode-gpui/src/visuals"]
mod drawing {
    mod scene;
    use super::shared::visual::{Node, ThemeColor};
    use serde_json::{json, Value};

    /// A scene node seen from an orbit, as polygons and strokes in draw
    /// order, in the scene's own width and height.
    pub fn project_scene(node: &Value, yaw: f32, pitch: f32, zoom: f32) -> Result<Value, String> {
        let node: Node = serde_json::from_value(node.clone()).map_err(|e| e.to_string())?;
        let Node::Scene {
            width,
            height,
            objects,
            camera,
        } = node
        else {
            return Err("Not a scene".into());
        };
        let orbit = scene::Orbit {
            yaw,
            pitch: pitch.clamp(-1.4, 1.4),
            zoom: zoom.clamp(0.3, 4.),
        };
        let projection = scene::project(&objects, &camera, orbit, width, height);
        let color = |color: ThemeColor| serde_json::to_value(color).unwrap_or_default();
        let order: Vec<Value> = projection
            .order
            .iter()
            .map(|surface| match surface {
                scene::Surface::Face(i) => {
                    let face = &projection.faces[*i];
                    json!({"points": face.points, "color": color(face.color), "light": face.light})
                }
                scene::Surface::Stroke(i) => {
                    let stroke = &projection.strokes[*i];
                    json!({"points": stroke.points, "color": color(stroke.color), "width": stroke.width})
                }
            })
            .collect();
        Ok(json!({ "surfaces": order }))
    }
}
#[cfg(not(target_os = "android"))]
pub use drawing::project_scene;

#[cfg(target_os = "android")]
pub fn project_scene(_: &Value, _: f32, _: f32, _: f32) -> Result<Value, String> {
    Err("Interactive visuals open on desktop".into())
}

/// Runtimes kept alive at once. An evicted visual starts again from its
/// initial state the next time it is drawn, as on the desktop.
const LIVE: usize = 4;

enum Job {
    /// The current tree, starting the program if it is not running.
    Render { key: String, source: String },
    /// A control changed; the tree after its handler ran.
    Dispatch {
        key: String,
        source: String,
        handler: String,
        value: Value,
    },
    /// The program from its initial state.
    Reset { key: String, source: String },
    /// Stop every runtime whose key starts with this prefix.
    Drop { prefix: String },
}

type Reply = mpsc::Sender<Value>;

fn worker() -> &'static Mutex<mpsc::Sender<(Job, Option<Reply>)>> {
    static WORKER: OnceLock<Mutex<mpsc::Sender<(Job, Option<Reply>)>>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (send, receive) = mpsc::channel::<(Job, Option<Reply>)>();
        std::thread::Builder::new()
            .name("monocode-visuals".into())
            .spawn(move || {
                let mut live = Live::default();
                while let Ok((job, reply)) = receive.recv() {
                    let result = live.run(job);
                    if let Some(reply) = reply {
                        let _ = reply.send(result);
                    }
                }
            })
            .expect("Visual worker thread");
        Mutex::new(send)
    })
}

fn ask(job: Job) -> Value {
    let (send, receive) = mpsc::channel();
    if worker().lock().unwrap().send((job, Some(send))).is_err() {
        return json!({"error": "Visuals are unavailable"});
    }
    receive
        .recv()
        .unwrap_or_else(|_| json!({"error": "Visuals are unavailable"}))
}

/// What a visual row draws: `{"tree": …}` or `{"error": …}`.
pub fn render(key: &str, source: &str) -> Value {
    ask(Job::Render {
        key: key.into(),
        source: source.into(),
    })
}

pub fn dispatch(key: &str, source: &str, handler: &str, value: Value) -> Value {
    ask(Job::Dispatch {
        key: key.into(),
        source: source.into(),
        handler: handler.into(),
        value,
    })
}

pub fn reset(key: &str, source: &str) -> Value {
    ask(Job::Reset {
        key: key.into(),
        source: source.into(),
    })
}

/// Stops a transcript's runtimes without waiting.
pub fn forget(prefix: &str) {
    let _ = worker().lock().unwrap().send((
        Job::Drop {
            prefix: prefix.into(),
        },
        None,
    ));
}

/// Whether this build can run visuals. Android shows the desktop's notice.
pub const SUPPORTED: bool = cfg!(not(target_os = "android"));

#[cfg(not(target_os = "android"))]
#[derive(Default)]
struct Live {
    /// Key, the source it runs, and its runtime; most recently used last.
    runtimes: Vec<(String, String, shared::visual::VisualRuntime)>,
}

#[cfg(not(target_os = "android"))]
impl Live {
    fn run(&mut self, job: Job) -> Value {
        match job {
            Job::Render { key, source } => self.with(&key, &source, false, |runtime| runtime.render()),
            Job::Dispatch {
                key,
                source,
                handler,
                value,
            } => {
                // A program started again for this event has none of the state
                // the control belonged to: show it as it starts instead.
                let fresh = !self.has(&key, &source);
                self.with(&key, &source, false, |runtime| {
                    if fresh {
                        runtime.render()
                    } else {
                        runtime.dispatch(&handler, value)
                    }
                })
            }
            Job::Reset { key, source } => self.with(&key, &source, true, |runtime| runtime.render()),
            Job::Drop { prefix } => {
                self.runtimes.retain(|(key, _, _)| !key.starts_with(&prefix));
                Value::Null
            }
        }
    }

    fn has(&self, key: &str, source: &str) -> bool {
        self.runtimes
            .iter()
            .any(|(k, s, _)| k == key && s == source)
    }

    fn with(
        &mut self,
        key: &str,
        source: &str,
        restart: bool,
        call: impl FnOnce(
            &mut shared::visual::VisualRuntime,
        ) -> Result<shared::visual::Node, String>,
    ) -> Value {
        let found = self
            .runtimes
            .iter()
            .position(|(k, s, _)| k == key && s == source && !restart);
        let mut entry = match found {
            Some(index) => self.runtimes.remove(index),
            None => {
                self.runtimes.retain(|(k, _, _)| k != key);
                match shared::visual::VisualRuntime::new(source) {
                    Ok(runtime) => (key.to_owned(), source.to_owned(), runtime),
                    Err(error) => return json!({ "error": error }),
                }
            }
        };
        let result = call(&mut entry.2);
        if self.runtimes.len() >= LIVE {
            self.runtimes.remove(0);
        }
        self.runtimes.push(entry);
        match result.and_then(|node| serde_json::to_value(node).map_err(|e| e.to_string())) {
            Ok(tree) => json!({ "tree": tree }),
            Err(error) => json!({ "error": error }),
        }
    }
}

#[cfg(target_os = "android")]
#[derive(Default)]
struct Live;

#[cfg(target_os = "android")]
impl Live {
    fn run(&mut self, _: Job) -> Value {
        json!({"error": "Interactive visuals open on desktop"})
    }
}

/// Keys are unique per transcript, so two open conversations never share a
/// runtime.
pub fn key(transcript: u64, row: &str) -> String {
    format!("{transcript}:{row}")
}

#[cfg(all(test, not(target_os = "android")))]
mod tests {
    use super::*;

    const COUNTER: &str = r#"const state = visual.state({ n: 1 });
return () => visual.column([
  visual.text("Count " + state.n),
  visual.button("Add", () => { state.n += 1; }),
]);"#;

    fn button(tree: &Value) -> String {
        tree["tree"]["children"][1]["handler"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn a_control_runs_on_the_phone_and_keeps_its_state_until_reset() {
        let first = render("t1:a", COUNTER);
        assert_eq!(first["tree"]["children"][0]["text"], "Count 1");
        let handler = button(&first);
        let next = dispatch("t1:a", COUNTER, &handler, Value::Null);
        assert_eq!(next["tree"]["children"][0]["text"], "Count 2");
        assert_eq!(
            render("t1:a", COUNTER)["tree"]["children"][0]["text"],
            "Count 2"
        );
        assert_eq!(
            reset("t1:a", COUNTER)["tree"]["children"][0]["text"],
            "Count 1"
        );
        forget("t1:");
    }

    #[test]
    fn a_stale_or_invalid_event_is_an_error_not_a_crash() {
        let first = render("t2:a", COUNTER);
        let wrong = dispatch("t2:a", COUNTER, &button(&first), json!(5));
        assert!(wrong["error"].as_str().unwrap().contains("Invalid value"));
        let broken = render("t2:b", "return () => visual.nope();");
        assert!(broken["error"].is_string());
        forget("t2:");
    }

    #[test]
    fn a_scene_projects_to_polygons_that_change_with_the_orbit() {
        let drawn = render(
            "t4:a",
            "return () => visual.scene([visual.object({ kind: 'box' }, { color: 'accent' })], { height: 240 });",
        );
        let scene = &drawn["tree"];
        let front = project_scene(scene, 0., 0., 1.).unwrap();
        let turned = project_scene(scene, 0.8, 0.3, 1.).unwrap();
        let faces = front["surfaces"].as_array().unwrap();
        assert!(!faces.is_empty());
        assert_eq!(faces[0]["color"], "accent");
        assert!(faces[0]["points"].as_array().unwrap().len() >= 3);
        assert_ne!(front, turned);
        assert!(project_scene(&json!({"kind":"text"}), 0., 0., 1.).is_err());
        forget("t4:");
    }

    #[test]
    fn an_evicted_visual_starts_again_instead_of_applying_an_old_event() {
        let first = render("t3:a", COUNTER);
        let handler = button(&first);
        for n in 0..LIVE {
            render(&format!("t3:other{n}"), COUNTER);
        }
        let after = dispatch("t3:a", COUNTER, &handler, Value::Null);
        assert_eq!(after["tree"]["children"][0]["text"], "Count 1");
        forget("t3:");
    }
}
