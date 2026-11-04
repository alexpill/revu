//! GitHub access through the `gh` CLI, so auth/hosts/tokens are gh's business.
//!
//! Every function here blocks: callers run them on background threads.

pub mod model;
pub mod mutation;

use anyhow::{Context, Result, anyhow, bail};
use model::*;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

fn gh(args: &[&str], stdin: Option<&str>) -> Result<Vec<u8>> {
    let mut child = Command::new("gh")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to run `gh` (is the GitHub CLI installed?)")?;
    if let Some(input) = stdin {
        child.stdin.take().unwrap().write_all(input.as_bytes())?;
    }
    drop(child.stdin.take());
    let out = child.wait_with_output()?;
    if !out.status.success() {
        // GraphQL errors come back on stdout as JSON, everything else on stderr.
        if let Ok(v) = serde_json::from_slice::<Value>(&out.stdout) {
            if let Some(msg) = graphql_errors(&v) {
                bail!(msg);
            }
        }
        bail!("gh {}: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

fn graphql_errors(v: &Value) -> Option<String> {
    let errs = v.get("errors")?.as_array()?;
    let msgs: Vec<_> = errs.iter().filter_map(|e| e["message"].as_str()).collect();
    Some(msgs.join("; "))
}

pub fn graphql(query: &str, variables: Value) -> Result<Value> {
    let input = json!({ "query": query, "variables": variables }).to_string();
    let args = ["api", "graphql", "--input", "-"];
    // GitHub's GraphQL gateway times out now and then on heavy queries.
    let out = gh(&args, Some(&input)).or_else(|e| match e.to_string().contains("HTTP 50") {
        true => gh(&args, Some(&input)),
        false => Err(e),
    })?;
    let mut v: Value = serde_json::from_slice(&out)?;
    if let Some(msg) = graphql_errors(&v) {
        bail!(msg);
    }
    Ok(v["data"].take())
}

fn decode<T: DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).context("unexpected GitHub response shape")
}

/// Raw REST GET (e.g. file contents with the raw media type).
fn rest_raw(path: &str, accept: &str) -> Result<Vec<u8>> {
    gh(&["api", "-H", &format!("Accept: {accept}"), path], None)
}

fn rest_paginated<T: DeserializeOwned>(path: &str) -> Result<Vec<T>> {
    let out = gh(&["api", "--paginate", "--slurp", path], None)?;
    let pages: Vec<Vec<T>> = serde_json::from_slice(&out).context("unexpected GitHub response shape")?;
    Ok(pages.into_iter().flatten().collect())
}

const PR_FIELDS: &str = r#"
fragment PrFields on PullRequest {
  number title isDraft updatedAt headRefName reviewDecision additions deletions
  author { login }
  assignees(first: 10) { nodes { login } }
  reviewRequests(first: 20) { nodes { requestedReviewer {
    ... on User { login } ... on Team { login: slug } ... on Bot { login } } } }
  commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
}"#;

/// The most recently updated open PRs of a repo.
// shortcut: first 30 only (GitHub gets slow and 502s on bigger pages), add paging if a repo needs more.
pub fn repo_prs(owner: &str, name: &str) -> Result<RepoPrs> {
    let q = format!(
        "query($owner: String!, $name: String!) {{ viewer {{ login }} repository(owner: $owner, name: $name) {{
          pullRequests(states: OPEN, first: 30, orderBy: {{field: UPDATED_AT, direction: DESC}}) {{
            totalCount nodes {{ ...PrFields }} }} }} }}{PR_FIELDS}"
    );
    let mut data = graphql(&q, json!({ "owner": owner, "name": name }))?;
    let mut prs: RepoPrs = decode(data["repository"]["pullRequests"].take())?;
    prs.viewer = data["viewer"]["login"].as_str().unwrap_or_default().to_string();
    Ok(prs)
}

/// `query` uses GitHub search syntax and is scoped to the repo.
pub fn issues(repo: &str, query: &str) -> Result<Vec<IssueSummary>> {
    const Q: &str = r#"query($q: String!) { search(query: $q, type: ISSUE, first: 50) { nodes { ... on Issue {
      number title state updatedAt author { login } labels(first: 5) { nodes { name } } comments { totalCount } } } } }"#;
    let mut data = graphql(Q, json!({ "q": format!("repo:{repo} is:issue {query}") }))?;
    decode(data["search"]["nodes"].take())
}

pub fn issue(owner: &str, name: &str, number: u64) -> Result<IssueDetail> {
    const Q: &str = r#"query($owner: String!, $name: String!, $number: Int!) { repository(owner: $owner, name: $name) {
      issue(number: $number) { id number title body url state createdAt author { login }
        labels(first: 20) { nodes { name } } assignees(first: 10) { nodes { login } }
        comments(first: 100) { nodes { author { login } body createdAt } } } } }"#;
    let mut data = graphql(Q, json!({ "owner": owner, "name": name, "number": number }))?;
    decode(data["repository"]["issue"].take())
}

pub fn pr(owner: &str, name: &str, number: u64) -> Result<PrDetail> {
    const Q: &str = r#"query($owner: String!, $name: String!, $number: Int!) { repository(owner: $owner, name: $name) {
      pullRequest(number: $number) {
        id number title body url state isDraft createdAt baseRefName headRefName headRefOid
        additions deletions changedFiles mergeable reviewDecision
        author { login }
        assignees(first: 10) { nodes { login } }
        labels(first: 20) { nodes { name } }
        reviewRequests(first: 20) { nodes { requestedReviewer {
          ... on User { login } ... on Team { login: slug } ... on Bot { login } } } }
        commits(last: 1) { nodes { commit { statusCheckRollup { state contexts(first: 100) { nodes {
          ... on CheckRun { name status conclusion } ... on StatusContext { context state } } } } } } }
        comments(first: 100) { nodes { author { login } body createdAt } }
        reviews(first: 100) { nodes { author { login } state body submittedAt } }
        reviewThreads(first: 100) { nodes { id isResolved isOutdated path line originalLine diffSide
          comments(first: 50) { nodes { author { login } body createdAt state } } } }
      } } }"#;
    let mut data = graphql(Q, json!({ "owner": owner, "name": name, "number": number }))?;
    decode(data["repository"]["pullRequest"].take())
}

/// Changed files with their patches, plus GitHub's per-file "viewed" flag.
pub fn pr_files(owner: &str, name: &str, number: u64) -> Result<(Vec<ChangedFile>, HashMap<String, bool>)> {
    let files = rest_paginated(&format!("repos/{owner}/{name}/pulls/{number}/files?per_page=100"))?;
    Ok((files, viewed_states(owner, name, number)?))
}

fn viewed_states(owner: &str, name: &str, number: u64) -> Result<HashMap<String, bool>> {
    const Q: &str = r#"query($owner: String!, $name: String!, $number: Int!, $after: String) {
      repository(owner: $owner, name: $name) { pullRequest(number: $number) {
        files(first: 100, after: $after) { pageInfo { hasNextPage endCursor } nodes { path viewerViewedState } } } } }"#;
    let mut out = HashMap::new();
    let mut after = Value::Null;
    loop {
        let data = graphql(Q, json!({ "owner": owner, "name": name, "number": number, "after": after }))?;
        let files = &data["repository"]["pullRequest"]["files"];
        for n in files["nodes"].as_array().ok_or_else(|| anyhow!("no files"))? {
            if let Some(p) = n["path"].as_str() {
                out.insert(p.to_string(), n["viewerViewedState"] == "VIEWED");
            }
        }
        if files["pageInfo"]["hasNextPage"] != true {
            return Ok(out);
        }
        after = files["pageInfo"]["endCursor"].clone();
    }
}

/// All file paths at `oid` (used when there is no local clone).
pub fn tree(owner: &str, name: &str, oid: &str) -> Result<Vec<String>> {
    let out = gh(&["api", &format!("repos/{owner}/{name}/git/trees/{oid}?recursive=1")], None)?;
    let v: Value = serde_json::from_slice(&out)?;
    let entries = v["tree"].as_array().ok_or_else(|| anyhow!("no tree"))?;
    Ok(entries
        .iter()
        .filter(|e| e["type"] == "blob")
        .filter_map(|e| e["path"].as_str().map(String::from))
        .collect())
}

pub fn file(owner: &str, name: &str, oid: &str, path: &str) -> Result<String> {
    let path = encode_path(path);
    let out = rest_raw(&format!("repos/{owner}/{name}/contents/{path}?ref={oid}"), "application/vnd.github.raw")?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

fn encode_path(p: &str) -> String {
    let mut s = String::with_capacity(p.len());
    for b in p.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s += &format!("%{b:02X}");
        }
    }
    s
}

/// Opens a PR or issue page in the browser.
pub fn browse(repo: &str, number: u64) -> Result<()> {
    gh(&["browse", "-R", repo, &number.to_string()], None).map(drop)
}

#[cfg(test)]
mod tests {
    #[test]
    fn encode() {
        assert_eq!(super::encode_path("a b/c#d.rs"), "a%20b/c%23d.rs");
    }
}
