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

/// Both make gh color its JSON even into a pipe, which then doesn't parse.
fn command(args: &[&str]) -> Command {
    let mut cmd = Command::new("gh");
    cmd.args(args).env_remove("CLICOLOR_FORCE").env_remove("GH_FORCE_TTY");
    cmd
}

fn gh(args: &[&str], stdin: Option<&str>) -> Result<Vec<u8>> {
    let mut child = command(args)
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
        if let Ok(v) = serde_json::from_slice::<Value>(&out.stdout)
            && let Some(msg) = graphql_errors(&v)
        {
            bail!(msg);
        }
        bail!(
            "gh {}: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        );
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

const FIELDS: &str = r#"
fragment PrFields on PullRequest {
  id number title isDraft updatedAt headRefName reviewDecision
  repository { nameWithOwner mergeCommitAllowed squashMergeAllowed rebaseMergeAllowed }
  author { login }
  assignees(first: 10) { nodes { login } } labels(first: 20) { nodes { name } }
  reviewRequests(first: 20) { nodes { requestedReviewer {
    ... on User { login } ... on Team { login: slug } ... on Bot { login } } } }
  commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
}
fragment IssueFields on Issue {
  id number title updatedAt repository { nameWithOwner } author { login }
  assignees(first: 10) { nodes { login } } labels(first: 20) { nodes { name } }
}"#;

/// Open PRs and issues matching any of `scopes` (GitHub search qualifiers,
/// e.g. `involves:@me` or `repo:a/b repo:c/d`), in one request.
// shortcut: 50 PRs and 50 issues per scope, add paging if someone misses items.
pub fn search(scopes: &[String]) -> Result<Activity> {
    if scopes.is_empty() {
        return Ok(Activity {
            viewer: String::new(),
            items: Vec::new(),
        });
    }
    let (mut params, mut fields, mut vars) = (Vec::new(), String::new(), serde_json::Map::new());
    for (i, scope) in scopes.iter().enumerate() {
        for kind in ["pr", "issue"] {
            params.push(format!("${kind}{i}: String!"));
            fields += &format!(
                " {kind}{i}: search(query: ${kind}{i}, type: ISSUE, first: 50) {{ nodes {{ __typename ...PrFields ...IssueFields }} }}"
            );
            vars.insert(
                format!("{kind}{i}"),
                format!("is:{kind} is:open archived:false sort:updated-desc {scope}").into(),
            );
        }
    }
    let q = format!("query({}) {{ viewer {{ login }}{fields} }}{FIELDS}", params.join(", "));
    let mut data = graphql(&q, Value::Object(vars))?;
    let viewer = data["viewer"]["login"].as_str().unwrap_or_default().to_string();
    let mut items: Vec<(String, Item)> = Vec::new();
    for (key, v) in data.as_object_mut().into_iter().flatten() {
        if key == "viewer" {
            continue;
        }
        for mut n in std::mem::take(&mut v["nodes"])
            .as_array_mut()
            .into_iter()
            .flatten()
            .map(std::mem::take)
        {
            let repo = n["repository"]["nameWithOwner"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let item = match n["__typename"].as_str() {
                Some("PullRequest") => Item::Pr(decode(n.take())?),
                Some("Issue") => Item::Issue(decode(n.take())?),
                _ => continue,
            };
            // Scopes overlap (a pinned repo you're involved in).
            if !items.iter().any(|(r, i)| *r == repo && i.number() == item.number()) {
                items.push((repo, item));
            }
        }
    }
    items.sort_by(|a, b| b.1.updated_at().cmp(a.1.updated_at()));
    Ok(Activity { viewer, items })
}

/// Logins of the organizations the viewer belongs to.
pub fn orgs() -> Result<Vec<String>> {
    let data = graphql(
        "query { viewer { organizations(first: 100) { nodes { login } } } }",
        json!({}),
    )?;
    let nodes = data["viewer"]["organizations"]["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok(nodes
        .iter()
        .filter_map(|n| n["login"].as_str().map(String::from))
        .collect())
}

/// (id, name) of the repo's labels, or (id, login) of the users who can be
/// assigned or asked for a review.
// shortcut: the first 100, page if a repo has more.
pub fn choices(owner: &str, name: &str, users: bool) -> Result<Vec<(String, String)>> {
    let field = match users {
        true => "assignableUsers(first: 100) { nodes { id name: login } }",
        false => "labels(first: 100) { nodes { id name } }",
    };
    let q =
        format!("query($owner: String!, $name: String!) {{ repository(owner: $owner, name: $name) {{ {field} }} }}");
    let data = graphql(&q, json!({ "owner": owner, "name": name }))?;
    let repo = data["repository"]
        .as_object()
        .ok_or_else(|| anyhow!("repo not found"))?;
    let nodes = repo
        .values()
        .next()
        .and_then(|v| v["nodes"].as_array())
        .cloned()
        .unwrap_or_default();
    Ok(nodes
        .iter()
        .filter_map(|n| Some((n["id"].as_str()?.to_string(), n["name"].as_str()?.to_string())))
        .collect())
}

/// Clones `repo` into `dir` (gh picks the protocol the user set up).
pub fn clone(repo: &str, dir: &std::path::Path) -> Result<()> {
    gh(&["repo", "clone", repo, &dir.to_string_lossy(), "--", "--quiet"], None).map(drop)
}

pub fn issue(owner: &str, name: &str, number: u64) -> Result<IssueDetail> {
    const Q: &str = r#"query($owner: String!, $name: String!, $number: Int!) { repository(owner: $owner, name: $name) {
      issue(number: $number) { id title body state createdAt author { login }
        labels(first: 20) { nodes { name } } assignees(first: 10) { nodes { login } }
        comments(first: 100) { nodes { author { login } body createdAt } } } } }"#;
    let mut data = graphql(Q, json!({ "owner": owner, "name": name, "number": number }))?;
    decode(data["repository"]["issue"].take())
}

pub fn pr(owner: &str, name: &str, number: u64) -> Result<PrDetail> {
    const Q: &str = r#"query($owner: String!, $name: String!, $number: Int!) { repository(owner: $owner, name: $name) {
      pullRequest(number: $number) {
        id number title body state isDraft createdAt baseRefName headRefName headRefOid
        additions deletions changedFiles mergeable reviewDecision
        repository { mergeCommitAllowed squashMergeAllowed rebaseMergeAllowed }
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
          comments(first: 50) { nodes { id author { login } body createdAt state viewerCanDelete } } } }
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
        let data = graphql(
            Q,
            json!({ "owner": owner, "name": name, "number": number, "after": after }),
        )?;
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
    let out = gh(
        &["api", &format!("repos/{owner}/{name}/git/trees/{oid}?recursive=1")],
        None,
    )?;
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
    let out = rest_raw(
        &format!("repos/{owner}/{name}/contents/{path}?ref={oid}"),
        "application/vnd.github.raw",
    )?;
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
    fn gh_never_forced_to_color() {
        let cmd = super::command(&["api", "user"]);
        let envs: Vec<_> = cmd.get_envs().collect();
        for var in ["CLICOLOR_FORCE", "GH_FORCE_TTY"] {
            assert!(envs.contains(&(var.as_ref(), None)), "{var} not removed");
        }
    }

    #[test]
    fn encode() {
        assert_eq!(super::encode_path("a b/c#d.rs"), "a%20b/c%23d.rs");
    }
}
