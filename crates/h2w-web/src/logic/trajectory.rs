//! Split an executor trajectory into marked blocks for highlighting.
//!
//! harness2 trajectories interleave `[ACTION]`, `[OBSERVATION]` and `[ERROR]` sections. A
//! marker starts a block that runs until the next marker; text before the first marker is
//! plain. Markers may follow a short prefix such as a step number or timestamp.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrajKind {
    Action,
    Observation,
    Error,
    Plain,
}

impl TrajKind {
    pub(crate) fn class(self) -> &'static str {
        match self {
            TrajKind::Action => "traj-action",
            TrajKind::Observation => "traj-observation",
            TrajKind::Error => "traj-error",
            TrajKind::Plain => "traj-plain",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            TrajKind::Action => "Action",
            TrajKind::Observation => "Observation",
            TrajKind::Error => "Error",
            TrajKind::Plain => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrajBlock {
    pub(crate) kind: TrajKind,
    pub(crate) text: String,
}

const MARKERS: [(&str, TrajKind); 3] = [
    ("[ACTION]", TrajKind::Action),
    ("[OBSERVATION]", TrajKind::Observation),
    ("[ERROR]", TrajKind::Error),
];

/// The marker on a line, when it appears within the first 40 bytes.
fn marker(line: &str) -> Option<TrajKind> {
    MARKERS.iter().find_map(|(m, kind)| {
        line.find(m)
            .filter(|&i| i <= 40 && line[..i].trim().len() < 40)
            .map(|_| *kind)
    })
}

/// True when the text contains at least one trajectory marker at a line start.
pub(crate) fn looks_like_trajectory(text: &str) -> bool {
    text.lines().take(5_000).any(|l| marker(l).is_some())
}

/// True for file names harness2 uses for trajectories.
pub(crate) fn is_trajectory_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    name.contains("trajectory") || name.contains("transcript")
}

/// Group lines into blocks; consecutive lines of one block are joined with `\n`.
pub(crate) fn segment(text: &str) -> Vec<TrajBlock> {
    let mut blocks: Vec<TrajBlock> = Vec::new();
    for line in text.lines() {
        match marker(line) {
            Some(kind) => blocks.push(TrajBlock {
                kind,
                text: line.to_owned(),
            }),
            None => match blocks.last_mut() {
                Some(b) => {
                    b.text.push('\n');
                    b.text.push_str(line);
                }
                None => blocks.push(TrajBlock {
                    kind: TrajKind::Plain,
                    text: line.to_owned(),
                }),
            },
        }
    }
    blocks
}

/// Counts of `(actions, observations, errors)`.
pub(crate) fn counts(blocks: &[TrajBlock]) -> (usize, usize, usize) {
    blocks.iter().fold((0, 0, 0), |(a, o, e), b| match b.kind {
        TrajKind::Action => (a + 1, o, e),
        TrajKind::Observation => (a, o + 1, e),
        TrajKind::Error => (a, o, e + 1),
        TrajKind::Plain => (a, o, e),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_blocks() {
        let t = "task: summarise\n[ACTION] read_file a.md\nargs...\n[OBSERVATION] 120 lines\n\
                 more\n12:00:01 [ERROR] tool timeout\n[ACTION] retry";
        let b = segment(t);
        let kinds: Vec<TrajKind> = b.iter().map(|b| b.kind).collect();
        assert_eq!(
            kinds,
            vec![
                TrajKind::Plain,
                TrajKind::Action,
                TrajKind::Observation,
                TrajKind::Error,
                TrajKind::Action
            ]
        );
        assert_eq!(b[1].text, "[ACTION] read_file a.md\nargs...");
        assert_eq!(counts(&b), (2, 1, 1));
        assert!(looks_like_trajectory(t));
        assert!(!looks_like_trajectory("plain text\nnothing here"));
    }

    #[test]
    fn markers_far_into_a_line_are_not_markers() {
        let line = format!("{}[ACTION] quoted", "x".repeat(60));
        assert_eq!(segment(&line)[0].kind, TrajKind::Plain);
    }

    #[test]
    fn trajectory_paths() {
        assert!(is_trajectory_path("run/trajectory.md"));
        assert!(is_trajectory_path("Transcript.txt"));
        assert!(!is_trajectory_path("output/memo.md"));
    }
}
