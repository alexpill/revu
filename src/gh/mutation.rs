//! Write operations, executed one at a time on a dedicated thread.
//!
//! The UI applies changes optimistically and queues them here; running them
//! sequentially keeps rapid toggles (viewed/unviewed) in order on GitHub's side.

use super::graphql;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::sync::mpsc::{Sender, channel};

#[derive(Debug, Clone)]
pub enum Mutation {
    SetViewed { path: String, viewed: bool },
    /// Added to the viewer's pending review (created on demand).
    AddThread { path: String, line: u32, side: &'static str, start_line: Option<u32>, body: String },
    Reply { thread_id: String, body: String },
    SetResolved { thread_id: String, resolved: bool },
    /// event: COMMENT | APPROVE | REQUEST_CHANGES
    Submit { event: &'static str, body: String },
    DiscardReview,
    Comment { body: String },
}

impl Mutation {
    /// Whether the PR must be re-fetched to show the result.
    pub fn needs_reload(&self) -> bool {
        !matches!(self, Mutation::SetViewed { .. })
    }

    /// Fills in the text of a mutation drafted before the user wrote it.
    pub fn with_body(mut self, text: String) -> Self {
        if let Mutation::AddThread { body, .. }
        | Mutation::Reply { body, .. }
        | Mutation::Submit { body, .. }
        | Mutation::Comment { body } = &mut self
        {
            *body = text;
        }
        self
    }
}

pub struct Job<K> {
    pub key: K,
    pub pr_id: String,
    pub mutation: Mutation,
}

/// Starts the worker; `done` is called after each job with its outcome.
pub fn spawn_worker<K: Send + 'static>(done: impl Fn(Job<K>, Result<()>) + Send + 'static) -> Sender<Job<K>> {
    let (tx, rx) = channel::<Job<K>>();
    std::thread::spawn(move || {
        for job in rx {
            let res = run(&job.pr_id, &job.mutation);
            done(job, res);
        }
    });
    tx
}

fn run(pr: &str, m: &Mutation) -> Result<()> {
    match m {
        Mutation::SetViewed { path, viewed } => {
            let op = if *viewed { "markFileAsViewed" } else { "unmarkFileAsViewed" };
            let q = format!("mutation($pr: ID!, $path: String!) {{ {op}(input: {{pullRequestId: $pr, path: $path}}) {{ clientMutationId }} }}");
            graphql(&q, json!({ "pr": pr, "path": path }))?;
        }
        Mutation::AddThread { path, line, side, start_line, body } => {
            let review = pending_review(pr)?.map_or_else(|| create_review(pr), Ok)?;
            graphql(
                r#"mutation($review: ID!, $path: String!, $line: Int!, $side: DiffSide!, $startLine: Int, $startSide: DiffSide, $body: String!) {
                  addPullRequestReviewThread(input: {pullRequestReviewId: $review, path: $path, line: $line, side: $side,
                    startLine: $startLine, startSide: $startSide, body: $body, subjectType: LINE}) { thread { id } } }"#,
                json!({ "review": review, "path": path, "line": line, "side": side, "body": body,
                        "startLine": start_line, "startSide": start_line.map(|_| side) }),
            )?;
        }
        Mutation::Reply { thread_id, body } => {
            let review = pending_review(pr)?.map_or_else(|| create_review(pr), Ok)?;
            graphql(
                r#"mutation($thread: ID!, $review: ID!, $body: String!) {
                  addPullRequestReviewThreadReply(input: {pullRequestReviewThreadId: $thread, pullRequestReviewId: $review, body: $body}) { comment { id } } }"#,
                json!({ "thread": thread_id, "review": review, "body": body }),
            )?;
        }
        Mutation::SetResolved { thread_id, resolved } => {
            let op = if *resolved { "resolveReviewThread" } else { "unresolveReviewThread" };
            let q = format!("mutation($id: ID!) {{ {op}(input: {{threadId: $id}}) {{ clientMutationId }} }}");
            graphql(&q, json!({ "id": thread_id }))?;
        }
        Mutation::Submit { event, body } => match pending_review(pr)? {
            Some(review) => {
                graphql(
                    r#"mutation($review: ID!, $event: PullRequestReviewEvent!, $body: String) {
                      submitPullRequestReview(input: {pullRequestReviewId: $review, event: $event, body: $body}) { clientMutationId } }"#,
                    json!({ "review": review, "event": event, "body": body }),
                )?;
            }
            None => {
                graphql(
                    r#"mutation($pr: ID!, $event: PullRequestReviewEvent!, $body: String) {
                      addPullRequestReview(input: {pullRequestId: $pr, event: $event, body: $body}) { clientMutationId } }"#,
                    json!({ "pr": pr, "event": event, "body": body }),
                )?;
            }
        },
        Mutation::DiscardReview => {
            let review = pending_review(pr)?.ok_or_else(|| anyhow!("no pending review"))?;
            graphql(
                "mutation($id: ID!) { deletePullRequestReview(input: {pullRequestReviewId: $id}) { clientMutationId } }",
                json!({ "id": review }),
            )?;
        }
        Mutation::Comment { body } => {
            graphql(
                "mutation($id: ID!, $body: String!) { addComment(input: {subjectId: $id, body: $body}) { clientMutationId } }",
                json!({ "id": pr, "body": body }),
            )?;
        }
    }
    Ok(())
}

/// The viewer's unsubmitted review on this PR, if any.
fn pending_review(pr: &str) -> Result<Option<String>> {
    let data = graphql(
        r#"query($pr: ID!) { node(id: $pr) { ... on PullRequest {
          reviews(states: PENDING, first: 10) { nodes { id viewerDidAuthor } } } } }"#,
        json!({ "pr": pr }),
    )?;
    let nodes = data["node"]["reviews"]["nodes"].as_array().cloned().unwrap_or_default();
    Ok(nodes
        .iter()
        .find(|n| n["viewerDidAuthor"] == true)
        .and_then(|n| n["id"].as_str().map(String::from)))
}

fn create_review(pr: &str) -> Result<String> {
    let data = graphql(
        "mutation($pr: ID!) { addPullRequestReview(input: {pullRequestId: $pr}) { pullRequestReview { id } } }",
        json!({ "pr": pr }),
    )?;
    match &data["addPullRequestReview"]["pullRequestReview"]["id"] {
        Value::String(id) => Ok(id.clone()),
        _ => Err(anyhow!("could not create a pending review")),
    }
}
