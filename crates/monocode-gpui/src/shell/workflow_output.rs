//! `workflow.output`: the session of a running workflow step records the step's declared
//! outputs. The call carries no run id. The bridge has already checked the session's token, so
//! `call.session` is who is asking; the app finds the step whose active session that is.
//! Chat and terminal sessions both come through here (the caller is looked up in the session
//! summaries, not only among the open chats), and the answer is a receipt like any other mutating
//! app action.
use super::*;
use monocode_core::mono::bridge::Call;

#[cfg(test)]
mod tests;

impl Shell {
    pub(super) fn workflow_output_call(&mut self, call: Call, cx: &mut Context<Self>) {
        let Some(workflows) = self.workflows.clone() else {
            return self.finish_app_call(call, Err("Workflows are unavailable".into()), cx);
        };
        if !self.session_summaries.contains_key(&call.session) {
            return self.finish_app_call(call, Err("Unknown session.".into()), cx);
        }
        // Run files name a session by its persisted id, which is not always the local one.
        let (store, local) = (self.store.clone(), call.session);
        let persisted = cx
            .background_executor()
            .spawn(async move { locked(&store)?.persisted_session_id(local) });
        cx.spawn(async move |this, cx| {
            let persisted = persisted.await;
            let _ = this.update(cx, |this, cx| {
                let mut ids = vec![local.to_string()];
                ids.extend(persisted.ok().filter(|id| *id != local.to_string()));
                let result = workflows.update(cx, |w, _| w.accept_output(&ids, &call.input));
                this.finish_app_call(call, result, cx);
            });
        })
        .detach();
    }
}
