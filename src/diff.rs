//! Unified-diff hunks (as GitHub's per-file `patch`) to numbered lines.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Hunk,
    Context,
    Add,
    Del,
    /// "\ No newline at end of file"
    Meta,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: Kind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    /// Without the leading +/-/space marker (full header for hunks).
    pub text: String,
}

impl DiffLine {
    /// Side and line number GitHub uses to anchor review comments.
    pub fn anchor(&self) -> Option<(&'static str, u32)> {
        match self.kind {
            Kind::Del => self.old.map(|n| ("LEFT", n)),
            Kind::Add | Kind::Context => self.new.map(|n| ("RIGHT", n)),
            _ => None,
        }
    }
}

pub fn parse(patch: &str) -> Vec<DiffLine> {
    let (mut old, mut new) = (0u32, 0u32);
    let mut out = Vec::new();
    for raw in patch.lines() {
        let line = |kind, o, n, text: &str| DiffLine {
            kind,
            old: o,
            new: n,
            text: text.to_string(),
        };
        if raw.starts_with("@@") {
            // @@ -a,b +c,d @@ section
            let mut nums = raw
                .split(' ')
                .skip(1)
                .take(2)
                .map(|r| r[1..].split(',').next().and_then(|n| n.parse().ok()).unwrap_or(0));
            old = nums.next().unwrap_or(0);
            new = nums.next().unwrap_or(0);
            out.push(line(Kind::Hunk, None, None, raw));
        } else if let Some(t) = raw.strip_prefix('+') {
            out.push(line(Kind::Add, None, Some(new), t));
            new += 1;
        } else if let Some(t) = raw.strip_prefix('-') {
            out.push(line(Kind::Del, Some(old), None, t));
            old += 1;
        } else if raw.starts_with('\\') {
            out.push(line(Kind::Meta, None, None, raw));
        } else {
            let t = raw.strip_prefix(' ').unwrap_or(raw);
            out.push(line(Kind::Context, Some(old), Some(new), t));
            old += 1;
            new += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_lines() {
        let d = parse("@@ -10,3 +10,4 @@ fn x()\n a\n-b\n+c\n+d\n e\n\\ No newline at end of file");
        let nums: Vec<_> = d.iter().map(|l| (l.kind, l.old, l.new)).collect();
        assert_eq!(
            nums,
            vec![
                (Kind::Hunk, None, None),
                (Kind::Context, Some(10), Some(10)),
                (Kind::Del, Some(11), None),
                (Kind::Add, None, Some(11)),
                (Kind::Add, None, Some(12)),
                (Kind::Context, Some(12), Some(13)),
                (Kind::Meta, None, None),
            ]
        );
        assert_eq!(d[2].anchor(), Some(("LEFT", 11)));
        assert_eq!(d[5].anchor(), Some(("RIGHT", 13)));
        // new-file hunk: "-0,0"
        assert_eq!(parse("@@ -0,0 +1 @@\n+x")[1].new, Some(1));
    }
}
