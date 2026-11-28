//! The extraction speaks with the voice of the session it reads: its harness, model, effort and
//! account. No provider runs: the voice is what the call would be made with, and the transcript
//! of a session whose prompt was never sent is empty.
use super::*;
use crate::workflows::extract::{Voice, extract};
use monocode_core::mono::workflows::{model::OutputField, ordered::OrderedMap};

fn voice_of(s: &Setup, cx: &mut TestAppContext, started: &StartedSession) -> Result<Voice, String> {
    let workflows = workflows_of(s, cx);
    let persisted = started.session_id.clone();
    cx.update(|cx| workflows.update(cx, |w, cx| w.extraction_voice(&persisted, cx)))
}

#[gpui::test]
fn the_voice_is_the_harness_model_and_effort_of_the_session(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut req = request(&s);
    req.harness = Some("claude".into());
    req.model = Some("claude:sonnet-5".into());
    req.effort = Some("high".into());
    let started = begin(&s, cx, req).unwrap();
    let voice = voice_of(&s, cx, &started).unwrap();
    assert_eq!(voice.kind, Kind::Claude);
    assert_eq!(voice.model.as_deref(), Some("claude-sonnet-5"));
    assert_eq!(voice.effort.as_deref(), Some("high"));
    assert_eq!(voice.local, local(&started));
    assert_eq!(voice.cwd, std::path::PathBuf::from(&s.project));

    // Another session of another harness is read with its own, not with the first one's.
    let mut other = request(&s);
    other.harness = Some("codex".into());
    other.model = Some("codex:test".into());
    probe_with(&s, cx, only_test_model);
    let second = begin(&s, cx, other).unwrap();
    let voice = voice_of(&s, cx, &second).unwrap();
    assert_eq!(voice.kind, Kind::Codex);
    assert_eq!(voice.model.as_deref(), Some("test"));
}

#[gpui::test]
fn the_voice_of_a_session_the_app_does_not_know_is_an_error(cx: &mut TestAppContext) {
    let s = setup(cx);
    let workflows = workflows_of(&s, cx);
    let error = cx
        .update(|cx| workflows.update(cx, |w, cx| w.extraction_voice("999999", cx)))
        .unwrap_err();
    assert!(error.contains("no longer available"), "{error}");
}

#[gpui::test]
fn a_conversation_with_nothing_in_it_is_not_sent_anywhere(cx: &mut TestAppContext) {
    let s = setup(cx);
    let started = begin(&s, cx, request(&s)).unwrap();
    let voice = voice_of(&s, cx, &started);
    let mut fields = OrderedMap::new();
    fields.insert(
        "summary",
        OutputField::new(monocode_core::mono::workflows::model::OutputFieldType::String),
    );
    let executor = cx.executor();
    let store = s.store.clone();
    let answer = collect(
        cx,
        Box::pin(extract(
            voice,
            store,
            ExtractRequest {
                session_id: started.session_id.clone(),
                fields,
                timeout_ms: 1_000,
            },
            executor,
        )),
    );
    cx.run_until_parked();
    let result = answer.borrow_mut().take().expect("it answered");
    let error = result.unwrap_err();
    assert!(error.contains("empty"), "{error}");
}
