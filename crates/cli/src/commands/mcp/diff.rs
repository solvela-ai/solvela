/// Produce a simple unified-style diff between two strings.
/// Each line is prefixed with ' ' (unchanged), '-' (removed), or '+' (added).
/// No external crate dependency — pure line comparison.
pub fn line_diff(old: &str, new: &str) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    // Use a simple LCS-based diff via a patience-like algorithm.
    // For the small JSON files we deal with, a naive O(n²) approach is fine.
    let hunks = compute_diff(&old_lines, &new_lines);

    let mut out = String::new();
    for hunk in &hunks {
        match hunk {
            DiffOp::Equal(line) => {
                out.push(' ');
                out.push_str(line);
                out.push('\n');
            }
            DiffOp::Delete(line) => {
                out.push('-');
                out.push_str(line);
                out.push('\n');
            }
            DiffOp::Insert(line) => {
                out.push('+');
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

#[derive(Debug)]
enum DiffOp<'a> {
    Equal(&'a str),
    Delete(&'a str),
    Insert(&'a str),
}

/// Compute the diff between two line slices using the classic LCS algorithm.
fn compute_diff<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<DiffOp<'a>> {
    let m = old.len();
    let n = new.len();

    // Build LCS table.
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            dp[i][j] = if old[i] == new[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    // Trace back.
    let mut ops = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < m || j < n {
        if i < m && j < n && old[i] == new[j] {
            ops.push(DiffOp::Equal(old[i]));
            i += 1;
            j += 1;
        } else if j < n && (i >= m || dp[i][j + 1] >= dp[i + 1][j]) {
            ops.push(DiffOp::Insert(new[j]));
            j += 1;
        } else {
            ops.push(DiffOp::Delete(old[i]));
            i += 1;
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_strings_all_equal() {
        let s = "line1\nline2\nline3";
        let d = line_diff(s, s);
        for line in d.lines() {
            assert!(
                line.starts_with(' '),
                "equal lines should have ' ' prefix: {line}"
            );
        }
    }

    #[test]
    fn test_addition_only() {
        let old = "a\nb";
        let new = "a\nb\nc";
        let d = line_diff(old, new);
        assert!(d.contains("+c"), "should show +c in diff");
    }

    #[test]
    fn test_deletion_only() {
        let old = "a\nb\nc";
        let new = "a\nb";
        let d = line_diff(old, new);
        assert!(d.contains("-c"), "should show -c in diff");
    }

    #[test]
    fn test_replacement() {
        let old = "key: old_value";
        let new = "key: new_value";
        let d = line_diff(old, new);
        assert!(d.contains("-key: old_value"), "should delete old line");
        assert!(d.contains("+key: new_value"), "should insert new line");
    }

    #[test]
    fn test_empty_old() {
        let d = line_diff("", "a\nb");
        assert!(d.contains("+a"));
        assert!(d.contains("+b"));
    }

    #[test]
    fn test_empty_new() {
        let d = line_diff("a\nb", "");
        assert!(d.contains("-a"));
        assert!(d.contains("-b"));
    }
}
