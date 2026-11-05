//! Typed views of the GraphQL/REST payloads we request.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Nodes<T> {
    pub nodes: Vec<T>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Actor {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Label {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewRequest {
    /// `None` for reviewer kinds we don't query (e.g. mannequins).
    pub requested_reviewer: Option<Actor>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommitNode {
    pub commit: Commit,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub status_check_rollup: Option<Rollup>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rollup {
    /// SUCCESS | FAILURE | ERROR | PENDING | EXPECTED
    pub state: String,
    pub contexts: Option<Nodes<Check>>,
}

/// Union of CheckRun and StatusContext, flattened.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub name: Option<String>,
    pub context: Option<String>,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub state: Option<String>,
}

impl Check {
    pub fn label(&self) -> &str {
        self.name.as_deref().or(self.context.as_deref()).unwrap_or("?")
    }
    /// SUCCESS / FAILURE / PENDING-ish, normalized from both check kinds.
    pub fn result(&self) -> &str {
        match (
            self.conclusion.as_deref(),
            self.state.as_deref(),
            self.status.as_deref(),
        ) {
            (Some(c), _, _) => c,
            (None, Some(s), _) => s,
            (None, None, Some(_)) => "PENDING",
            _ => "?",
        }
    }
}

/// The merge methods a repo allows.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Merges {
    pub merge_commit_allowed: bool,
    pub squash_merge_allowed: bool,
    pub rebase_merge_allowed: bool,
}

fn rollup(commits: &Nodes<CommitNode>) -> Option<&Rollup> {
    commits.nodes.first()?.commit.status_check_rollup.as_ref()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrSummary {
    pub id: String,
    pub number: u64,
    pub title: String,
    pub is_draft: bool,
    pub updated_at: String,
    pub head_ref_name: String,
    pub review_decision: Option<String>,
    pub author: Option<Actor>,
    pub assignees: Nodes<Actor>,
    pub review_requests: Nodes<ReviewRequest>,
    pub labels: Nodes<Label>,
    pub commits: Nodes<CommitNode>,
    pub repository: Merges,
}

impl PrSummary {
    pub fn ci(&self) -> Option<&str> {
        rollup(&self.commits).map(|r| r.state.as_str())
    }
    pub fn author_is(&self, login: &str) -> bool {
        self.author.as_ref().is_some_and(|a| a.login == login)
    }
    pub fn assigned_to(&self, login: &str) -> bool {
        self.assignees.nodes.iter().any(|a| a.login == login)
    }
    /// Only direct user requests: team membership isn't resolved.
    pub fn review_requested(&self, login: &str) -> bool {
        self.review_requests
            .nodes
            .iter()
            .any(|r| r.requested_reviewer.as_ref().is_some_and(|a| a.login == login))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueSummary {
    pub id: String,
    pub number: u64,
    pub title: String,
    pub updated_at: String,
    pub author: Option<Actor>,
    pub assignees: Nodes<Actor>,
    pub labels: Nodes<Label>,
}

pub enum Item {
    Pr(PrSummary),
    Issue(IssueSummary),
}

impl Item {
    pub fn number(&self) -> u64 {
        match self {
            Item::Pr(p) => p.number,
            Item::Issue(i) => i.number,
        }
    }
    pub fn updated_at(&self) -> &str {
        match self {
            Item::Pr(p) => &p.updated_at,
            Item::Issue(i) => &i.updated_at,
        }
    }
}

/// Search results: (owner/name, item), most recently updated first.
pub struct Activity {
    pub viewer: String,
    pub items: Vec<(String, Item)>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub author: Option<Actor>,
    pub body: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueDetail {
    pub id: String,
    pub title: String,
    pub body: String,
    pub state: String,
    pub created_at: String,
    pub author: Option<Actor>,
    pub labels: Nodes<Label>,
    pub assignees: Nodes<Actor>,
    pub comments: Nodes<Comment>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub author: Option<Actor>,
    /// APPROVED | CHANGES_REQUESTED | COMMENTED | DISMISSED | PENDING
    pub state: String,
    pub body: String,
    pub submitted_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadComment {
    pub id: String,
    pub author: Option<Actor>,
    pub body: String,
    pub created_at: String,
    /// PENDING while part of an unsubmitted review.
    pub state: String,
    pub viewer_can_delete: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: String,
    pub is_resolved: bool,
    pub is_outdated: bool,
    pub path: String,
    /// Line on the current diff; `None` once outdated.
    pub line: Option<u32>,
    pub original_line: Option<u32>,
    /// LEFT (old side) | RIGHT (new side)
    pub diff_side: String,
    pub comments: Nodes<ThreadComment>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrDetail {
    pub id: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    /// OPEN | CLOSED | MERGED
    pub state: String,
    pub is_draft: bool,
    pub created_at: String,
    pub base_ref_name: String,
    pub head_ref_name: String,
    pub head_ref_oid: String,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    /// MERGEABLE | CONFLICTING | UNKNOWN
    pub mergeable: String,
    pub review_decision: Option<String>,
    pub author: Option<Actor>,
    pub assignees: Nodes<Actor>,
    pub labels: Nodes<Label>,
    pub review_requests: Nodes<ReviewRequest>,
    pub commits: Nodes<CommitNode>,
    pub comments: Nodes<Comment>,
    pub reviews: Nodes<Review>,
    pub review_threads: Nodes<Thread>,
    pub repository: Merges,
}

impl PrDetail {
    pub fn rollup(&self) -> Option<&Rollup> {
        rollup(&self.commits)
    }
    pub fn pending_comments(&self) -> usize {
        let comments = self.review_threads.nodes.iter().flat_map(|t| &t.comments.nodes);
        comments.filter(|c| c.state == "PENDING").count()
    }
}

/// One changed file, from the REST `pulls/{n}/files` endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct ChangedFile {
    pub filename: String,
    pub previous_filename: Option<String>,
    /// added | removed | modified | renamed | copied | changed | unchanged
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    /// Missing for binary files and diffs GitHub deems too large.
    pub patch: Option<String>,
}
