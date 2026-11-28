//! The session step controls that decide what an agent may do: its access, the session folder it
//! is filed in, and whether it links the trigger item's work item.
use super::super::*;
use super::board::BoardPick;
use gpui::Div;
use monocode_core::mono::workflows::{
    board_edit::project_path,
    model::{DEFAULT_SESSION_ACCESS, Node, NodeKind, SESSION_ACCESS_IDS, Workflow},
};

/// What the folder picker calls the folder every run of the workflow shares.
pub(super) const WORKFLOW_FOLDER: &str = "Workflow folder";

pub(super) fn access_options() -> Vec<(String, String)> {
    SESSION_ACCESS_IDS
        .iter()
        .map(|id| {
            let label = match *id {
                "supervised" => "Supervised",
                "auto-accept-edits" => "Auto-accept edits",
                "auto" => "Auto",
                _ => "Full access",
            };
            (id.to_string(), label.to_string())
        })
        .collect()
}

/// Whether an event trigger leads to `id`.
pub(super) fn has_event_upstream(w: &Workflow, id: &str) -> bool {
    let mut seen = vec![id.to_string()];
    let mut at = 0;
    while at < seen.len() {
        let here = seen[at].clone();
        for edge in w.edges.iter().filter(|e| e.to == here) {
            if !seen.contains(&edge.from) {
                seen.push(edge.from.clone());
            }
        }
        at += 1;
    }
    seen.iter().skip(1).any(|n| {
        w.node(n)
            .is_some_and(|node| matches!(node.kind, NodeKind::Event(_)))
    })
}

impl Monos {
    pub(super) fn board_session_policy(
        &mut self,
        scope: &Scope,
        w: &Workflow,
        node: &Node,
        cx: &mut Context<Self>,
    ) -> Div {
        let NodeKind::Session {
            folder,
            link_work_item,
            target,
            ..
        } = &node.kind
        else {
            return div();
        };
        let id = node.id.clone();
        let local = target.is_none();
        let access = self.choice_row(
            "Access",
            BoardPick::Access(id.clone()),
            access_options(),
            node.kind.session_access(),
            cx,
        );
        let folder_row = local.then(|| {
            let project = project_path(w, node)
                .or_else(|| {
                    let own = w.workspace.project.trim();
                    (!own.is_empty() && !own.contains("{{")).then(|| own.to_string())
                })
                .unwrap_or_else(|| scope.project.clone());
            let current = folder.clone().unwrap_or_default();
            let mut options = vec![(String::new(), WORKFLOW_FOLDER.to_string())];
            options.extend(self.wf.folders.get(&project).cloned().unwrap_or_default());
            if !current.is_empty() && !options.iter().any(|(value, _)| *value == current) {
                options.push((current.clone(), "Missing folder".to_string()));
            }
            self.choice_row(
                "Folder",
                BoardPick::Folder(id.clone()),
                options,
                &current,
                cx,
            )
        });
        let link = (local && has_event_upstream(w, &id)).then(|| {
            let on = *link_work_item == Some(true);
            let toggled = id.clone();
            self.switch_row(
                "wf-link-work-item",
                "Link the work item",
                "File the trigger item's work item on this session. Nothing is written to the provider.",
                on,
                move |this, cx| {
                    this.board_node_edit(
                        &toggled,
                        |n| {
                            if let NodeKind::Session { link_work_item, .. } = &mut n.kind {
                                *link_work_item = (!on).then_some(true);
                            }
                        },
                        cx,
                    )
                },
                cx,
            )
        });
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_4()
                    .pt_3()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(access)
                    .children(folder_row),
            )
            .children(link.map(|l| div().px_2().child(l)))
    }

    pub(super) fn board_policy_picked(
        &mut self,
        pick: &BoardPick,
        value: &str,
        cx: &mut Context<Self>,
    ) {
        let v = value.to_string();
        match pick {
            BoardPick::Access(id) => self.board_node_edit(
                id,
                |n| {
                    if let NodeKind::Session { access, .. } = &mut n.kind {
                        // The default is left out of the file.
                        *access = (v != DEFAULT_SESSION_ACCESS).then(|| v.clone());
                    }
                },
                cx,
            ),
            BoardPick::Folder(id) => self.board_node_edit(
                id,
                |n| {
                    if let NodeKind::Session { folder, .. } = &mut n.kind {
                        *folder = (!v.is_empty()).then(|| v.clone());
                    }
                },
                cx,
            ),
            _ => {}
        }
    }
}
