//! Scoped, thread-local operation refusals for library tests. Neither the state
//! nor the selection contexts exist in production builds. Refusals happen after
//! the real operation, so recovery must discard completed/partial candidate data.
use std::cell::RefCell;

use super::{FormatEncodeError, ManifestTarget, MetadataSelection, Stage};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Fault {
    stage: Stage,
    memory: bool,
    require_scene: bool,
    frame: Option<u32>,
}
impl Fault {
    pub(crate) const fn at(stage: Stage) -> Self {
        Self {
            stage,
            memory: false,
            require_scene: false,
            frame: None,
        }
    }
    pub(crate) const fn allocation(mut self) -> Self {
        self.memory = true;
        self
    }
    pub(crate) fn with_scene(mut self) -> Self {
        assert!(matches!(
            self.stage,
            Stage::ManifestSerialization | Stage::ManifestEmbedding
        ));
        self.require_scene = true;
        self
    }
    pub(crate) const fn frame(mut self, ordinal: u32) -> Self {
        self.frame = Some(ordinal);
        self
    }
}
#[derive(Clone, Copy, Default)]
struct Input {
    scene: bool,
    frame: Option<u32>,
}
#[derive(Default)]
struct State {
    rules: Vec<Fault>,
    visits: Vec<Stage>,
    failures: Vec<Stage>,
}
std::thread_local! {
    static STATE:RefCell<Option<State>>=const {RefCell::new(None)};
    static INPUT:RefCell<Option<Input>>=const {RefCell::new(None)};
}
pub(super) struct Context(Option<Input>);
impl Drop for Context {
    fn drop(&mut self) {
        INPUT.with(|input| *input.borrow_mut() = self.0);
    }
}
pub(super) fn context(target: ManifestTarget<'_>, selection: MetadataSelection) -> Context {
    Context(INPUT.with(|input| {
        input.replace(Some(Input {
            scene: selection.embedded_scene,
            frame: match target {
                ManifestTarget::Scene => None,
                ManifestTarget::Frame { ordinal, .. } => Some(ordinal),
            },
        }))
    }))
}
pub(crate) struct Observed {
    pub visits: Vec<Stage>,
    pub failures: Vec<Stage>,
}
pub(crate) fn run<T>(rules: &[Fault], action: impl FnOnce() -> T) -> (T, Observed) {
    struct Restore(Option<State>);
    impl Drop for Restore {
        fn drop(&mut self) {
            STATE.with(|state| *state.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(STATE.with(|state| {
        state.replace(Some(State {
            rules: rules.to_vec(),
            ..Default::default()
        }))
    }));
    let value = action();
    let state = STATE.with(|state| state.borrow_mut().take().unwrap());
    (
        value,
        Observed {
            visits: state.visits,
            failures: state.failures,
        },
    )
}
pub(super) fn after(stage: Stage) -> Result<(), FormatEncodeError> {
    let Some(input) = INPUT.with(|input| *input.borrow()) else {
        return Ok(());
    };
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(state) = state.as_mut() else {
            return Ok(());
        };
        state.visits.push(stage);
        let rule = state
            .rules
            .iter()
            .find(|rule| {
                rule.stage == stage
                    && (!rule.require_scene || input.scene)
                    && rule.frame.is_none_or(|frame| Some(frame) == input.frame)
            })
            .copied();
        if let Some(rule) = rule {
            state.failures.push(stage);
            if rule.memory {
                Err(FormatEncodeError::MemoryUnavailable {
                    subject_kind: "metadata",
                    subject: "test_operation_refusal",
                })
            } else {
                Err(FormatEncodeError::Encoding(
                    "test metadata operation refusal".to_owned(),
                ))
            }
        } else {
            Ok(())
        }
    })
}
