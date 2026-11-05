//! Actions on a PR or an issue (labels, assignees, reviewers, draft / ready,
//! merge), shared by the home lists, the PR and the issue screens: a menu of
//! what the item and its repo allow, then a confirmation before any write.

use super::picker::{Picked, Picker};
use super::{Ctx, Key, Msg, ui};
use crate::gh::{self, model::*, mutation::Mutation};
use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::{Clear, Paragraph};

/// (key, GraphQL merge method, label); the uppercase key is the admin merge.
const METHODS: [(char, &str, &str); 3] = [
    ('m', "MERGE", "create a merge commit"),
    ('s', "SQUASH", "squash and merge"),
    ('b', "REBASE", "rebase and merge"),
];

/// What a PR has on top of an issue's actions.
pub struct Pr<'a> {
    pub open: bool,
    pub is_draft: bool,
    pub merges: &'a Merges,
    pub reviewers: &'a Nodes<ReviewRequest>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Edit {
    Labels,
    Assignees,
    Reviewers,
}

impl Edit {
    fn label(self) -> &'static str {
        match self {
            Edit::Labels => "labels",
            Edit::Assignees => "assignees",
            Edit::Reviewers => "reviewers",
        }
    }
}

enum Choice {
    /// Pick from the repo's labels or users first.
    Edit(Edit),
    Do(Mutation),
}

enum State {
    Choose,
    Loading(Edit),
    /// The picker shows the names of the (id, name) choices.
    Pick(Edit, Picker, Vec<(String, String)>),
    /// Waiting for `y`.
    Confirm(String, Mutation),
}

pub struct Menu {
    key: Key,
    /// Node id of the PR or issue.
    id: String,
    title: String,
    labels: Vec<String>,
    assignees: Vec<String>,
    reviewers: Vec<String>,
    choices: Vec<(char, String, Choice)>,
    state: State,
}

impl Menu {
    pub fn new(
        key: Key,
        id: &str,
        title: &str,
        labels: &Nodes<Label>,
        assignees: &Nodes<Actor>,
        pr: Option<Pr>,
    ) -> Self {
        let reviewers = pr.as_ref().map_or_else(Vec::new, |p| {
            let logins = p.reviewers.nodes.iter().filter_map(|r| r.requested_reviewer.as_ref());
            logins.map(|a| a.login.clone()).collect()
        });
        Self {
            key,
            id: id.to_string(),
            title: title.to_string(),
            labels: labels.nodes.iter().map(|l| l.name.clone()).collect(),
            assignees: assignees.nodes.iter().map(|a| a.login.clone()).collect(),
            reviewers,
            choices: choices(pr.as_ref()),
            state: State::Choose,
        }
    }

    fn set(&self, edit: Edit) -> &[String] {
        match edit {
            Edit::Labels => &self.labels,
            Edit::Assignees => &self.assignees,
            Edit::Reviewers => &self.reviewers,
        }
    }

    /// False once the menu is closed: Esc or an unknown key, or the confirmation answered.
    pub fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> bool {
        match &mut self.state {
            State::Pick(edit, picker, opts) => match picker.on_key(k) {
                Picked::Pending => true,
                Picked::Cancel => false,
                Picked::Pick(i) => {
                    let (edit, (id, name)) = (*edit, opts.swap_remove(i));
                    self.picked(edit, id, name, ctx)
                }
            },
            State::Confirm(label, m) => {
                if k.code == KeyCode::Char('y') {
                    ctx.mutate(self.key, &self.id, m.clone());
                    ctx.info(format!("{label}…"));
                } else {
                    ctx.info("cancelled");
                }
                false
            }
            State::Loading(_) => false,
            State::Choose => {
                let KeyCode::Char(c) = k.code else { return false };
                let Some((_, label, choice)) = self.choices.iter().find(|(key, ..)| *key == c) else {
                    return false;
                };
                match choice {
                    Choice::Edit(edit) => {
                        let (key, edit, (owner, name)) = (self.key, *edit, ctx.owner_name(self.key.0));
                        ctx.spawn(move || Msg::Choices(key, edit, gh::choices(&owner, &name, edit != Edit::Labels)));
                        self.state = State::Loading(edit);
                    }
                    Choice::Do(m) => {
                        let mut m = m.clone();
                        if let Mutation::Merge { delete_branch, .. } = &mut m {
                            *delete_branch = ctx.cfg.delete_branch;
                        }
                        self.state = State::Confirm(label.clone(), m);
                    }
                }
                true
            }
        }
    }

    /// The repo's labels or users, for the picker; false if the menu closes.
    pub fn on_choices(&mut self, key: Key, edit: Edit, r: Result<Vec<(String, String)>>, ctx: &mut Ctx) -> bool {
        if key != self.key || !matches!(self.state, State::Loading(e) if e == edit) {
            return true; // for a menu closed since
        }
        let opts = match r {
            Ok(opts) => opts,
            Err(e) => {
                ctx.error(e);
                return false;
            }
        };
        let set = self.set(edit);
        let items = opts
            .iter()
            .map(|(_, n)| format!("{} {n}", if set.contains(n) { "✓" } else { " " }))
            .collect();
        self.state = State::Pick(edit, Picker::new(edit.label(), items), opts);
        true
    }

    /// Picking what is already set removes it, except a review request (that takes the REST API).
    fn picked(&mut self, edit: Edit, id: String, name: String, ctx: &mut Ctx) -> bool {
        let on = self.set(edit).contains(&name);
        let (label, m) = match edit {
            Edit::Labels if on => (
                format!("remove label {name}"),
                Mutation::SetLabel { id, name, add: false },
            ),
            Edit::Labels => (format!("add label {name}"), Mutation::SetLabel { id, name, add: true }),
            Edit::Assignees if on => (
                format!("unassign {name}"),
                Mutation::SetAssignee {
                    id,
                    login: name,
                    add: false,
                },
            ),
            Edit::Assignees => (
                format!("assign {name}"),
                Mutation::SetAssignee {
                    id,
                    login: name,
                    add: true,
                },
            ),
            Edit::Reviewers if on => {
                ctx.info(format!("a review from {name} is already requested"));
                return false;
            }
            Edit::Reviewers => (
                format!("request a review from {name}"),
                Mutation::RequestReview { id, login: name },
            ),
        };
        self.state = State::Confirm(label, m);
        true
    }

    pub fn render(&mut self, f: &mut Frame, ctx: &Ctx) {
        let head = format!("{}#{}  {}", ctx.repo(self.key.0), self.key.1, self.title);
        let mut lines = vec![Line::from(head).dark_gray()];
        let title = match &mut self.state {
            State::Pick(_, picker, _) => return picker.render(f),
            State::Loading(edit) => {
                lines.push(Line::from(format!("loading {}…", edit.label())).dark_gray());
                "actions"
            }
            State::Choose => {
                lines.extend(
                    self.choices
                        .iter()
                        .map(|(k, label, _)| Line::from(vec![format!(" {k} ").cyan().bold(), label.clone().into()])),
                );
                "actions"
            }
            State::Confirm(label, m) => {
                lines.push(Line::from(format!("{label}?")).bold());
                if let Mutation::Merge {
                    admin, delete_branch, ..
                } = m
                {
                    if *admin {
                        lines.push(
                            Line::from("ADMIN MERGE: bypasses the branch protection rules")
                                .red()
                                .bold(),
                        );
                    }
                    if *delete_branch {
                        lines.push(Line::from("then delete its branch (delete_branch in the config)").yellow());
                    }
                }
                lines.push(Line::from(vec![
                    " y ".cyan().bold(),
                    "confirm · any other key: cancel".into(),
                ]));
                "confirm"
            }
        };
        let w = lines.iter().map(Line::width).max().unwrap_or(0) as u16 + 4;
        let area = ui::centered(f.area(), w, lines.len() as u16 + 2);
        f.render_widget(Clear, area);
        f.render_widget(Paragraph::new(lines).block(ui::block(title, true)), area);
    }
}

/// Labels and assignees for all; reviewers, draft / ready and the merges the
/// repo allows for an open PR (a draft can only be marked ready: GitHub won't merge it).
fn choices(pr: Option<&Pr>) -> Vec<(char, String, Choice)> {
    let mut out = vec![
        ('l', "labels…".into(), Choice::Edit(Edit::Labels)),
        ('a', "assignees…".into(), Choice::Edit(Edit::Assignees)),
    ];
    let Some(pr) = pr.filter(|p| p.open) else {
        return out;
    };
    out.push(('v', "request a review…".into(), Choice::Edit(Edit::Reviewers)));
    if pr.is_draft {
        out.push((
            'r',
            "mark ready for review".into(),
            Choice::Do(Mutation::SetDraft { draft: false }),
        ));
        return out;
    }
    out.push((
        'd',
        "convert to draft".into(),
        Choice::Do(Mutation::SetDraft { draft: true }),
    ));
    let allowed = [
        pr.merges.merge_commit_allowed,
        pr.merges.squash_merge_allowed,
        pr.merges.rebase_merge_allowed,
    ];
    let methods = METHODS.iter().zip(allowed).filter(|(_, ok)| *ok).map(|(m, _)| m);
    for admin in [false, true] {
        for &(key, method, label) in methods.clone() {
            // Set from the config when picked.
            let m = Mutation::Merge {
                method,
                admin,
                delete_branch: false,
            };
            out.push(match admin {
                false => (key, label.into(), Choice::Do(m)),
                true => (
                    key.to_ascii_uppercase(),
                    format!("admin: {label}, bypassing branch protection"),
                    Choice::Do(m),
                ),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_follow_the_item_and_repo() {
        let merges = |merge, squash, rebase| Merges {
            merge_commit_allowed: merge,
            squash_merge_allowed: squash,
            rebase_merge_allowed: rebase,
        };
        let reviewers = Nodes { nodes: Vec::new() };
        let keys = |open, is_draft, m: &Merges| {
            let pr = Pr {
                open,
                is_draft,
                merges: m,
                reviewers: &reviewers,
            };
            choices(Some(&pr)).into_iter().map(|(k, ..)| k).collect::<String>()
        };
        let all = merges(true, true, true);
        assert_eq!(choices(None).into_iter().map(|(k, ..)| k).collect::<String>(), "la");
        assert_eq!(keys(false, false, &all), "la");
        assert_eq!(keys(true, true, &all), "lavr");
        assert_eq!(keys(true, false, &all), "lavdmsbMSB");
        assert_eq!(keys(true, false, &merges(false, true, false)), "lavdsS");
        let c = choices(Some(&Pr {
            open: true,
            is_draft: false,
            merges: &merges(false, false, true),
            reviewers: &reviewers,
        }));
        assert!(matches!(
            c[5].2,
            Choice::Do(Mutation::Merge {
                method: "REBASE",
                admin: true,
                ..
            })
        ));
    }
}
