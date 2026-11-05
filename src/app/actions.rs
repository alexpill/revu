//! PR actions (draft / ready, merge), shared by the home lists and the PR
//! screen: a menu of what the PR and its repo allow, then a confirmation.

use super::{Ctx, Key, ui};
use crate::gh::{model::Merges, mutation::Mutation};
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

pub struct Menu {
    key: Key,
    pr_id: String,
    title: String,
    choices: Vec<(char, String, Mutation)>,
    /// The picked choice, waiting for `y`.
    confirm: Option<usize>,
}

impl Menu {
    pub fn new(key: Key, pr_id: &str, title: &str, is_draft: bool, merges: &Merges) -> Self {
        Self {
            key,
            pr_id: pr_id.to_string(),
            title: title.to_string(),
            choices: choices(is_draft, merges),
            confirm: None,
        }
    }

    /// Picks, then confirms with `y`; false once the menu is closed (any other key cancels).
    pub fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> bool {
        let KeyCode::Char(c) = k.code else { return false };
        match self.confirm {
            Some(i) if c == 'y' => {
                let (_, label, m) = &self.choices[i];
                ctx.mutate(self.key, &self.pr_id, m.clone());
                ctx.info(format!("{label}…"));
                false
            }
            Some(_) => {
                ctx.info("cancelled");
                false
            }
            None => {
                self.confirm = self.choices.iter().position(|(key, ..)| *key == c);
                self.confirm.is_some()
            }
        }
    }

    pub fn render(&self, f: &mut Frame, ctx: &Ctx) {
        let pr = format!("{}#{}", ctx.repo(self.key.0), self.key.1);
        let mut lines = vec![Line::from(format!("{pr}  {}", self.title)).dark_gray()];
        let title = match self.confirm {
            None => {
                lines.extend(
                    self.choices
                        .iter()
                        .map(|(k, label, _)| Line::from(vec![format!(" {k} ").cyan().bold(), label.clone().into()])),
                );
                "PR actions"
            }
            Some(i) => {
                let (_, label, m) = &self.choices[i];
                lines.push(Line::from(format!("{label} {pr}?")).bold());
                if matches!(m, Mutation::Merge { admin: true, .. }) {
                    lines.push(
                        Line::from("ADMIN MERGE: bypasses the branch protection rules")
                            .red()
                            .bold(),
                    );
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

/// What can be done to an open PR: a draft can only be marked ready (GitHub won't merge it).
fn choices(is_draft: bool, merges: &Merges) -> Vec<(char, String, Mutation)> {
    if is_draft {
        return vec![('r', "mark ready for review".into(), Mutation::SetDraft { draft: false })];
    }
    let allowed = [
        merges.merge_commit_allowed,
        merges.squash_merge_allowed,
        merges.rebase_merge_allowed,
    ];
    let methods = METHODS.iter().zip(allowed).filter(|(_, ok)| *ok).map(|(m, _)| m);
    let mut out = vec![('d', "convert to draft".into(), Mutation::SetDraft { draft: true })];
    for admin in [false, true] {
        for &(key, method, label) in methods.clone() {
            out.push(match admin {
                false => (key, label.into(), Mutation::Merge { method, admin }),
                true => (
                    key.to_ascii_uppercase(),
                    format!("admin: {label}, bypassing branch protection"),
                    Mutation::Merge { method, admin },
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
    fn choices_follow_the_repo() {
        let merges = |merge, squash, rebase| Merges {
            merge_commit_allowed: merge,
            squash_merge_allowed: squash,
            rebase_merge_allowed: rebase,
        };
        let keys = |c: Vec<(char, String, Mutation)>| c.into_iter().map(|(k, ..)| k).collect::<String>();
        assert_eq!(keys(choices(true, &merges(true, true, true))), "r");
        assert_eq!(keys(choices(false, &merges(true, true, true))), "dmsbMSB");
        assert_eq!(keys(choices(false, &merges(false, true, false))), "dsS");
        let c = choices(false, &merges(false, false, true));
        assert!(matches!(
            c[2].2,
            Mutation::Merge {
                method: "REBASE",
                admin: true
            }
        ));
    }
}
