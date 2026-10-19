//! The running subagents of a session as a tree under its row (doc/0029,
//! "Display"), shared by both styles: a count badge at the end of the name
//! column and up to [`MAX_AGENT_LINES`] child lines, then `… N more`.
//!
//! Child lines are drawn, never listed: they are not [`crate::app::Entry`]
//! values, so the selection, navigation and actions only ever see sessions
//! and group headers (doc/0006, doc/0011). A row has children only while
//! [`App::subagents_of`] returns agents for it.

use unicode_width::UnicodeWidthStr;

use super::fit;
use super::theme::rfit;
use crate::activity;
use crate::app::App;
use crate::merge::MergedSession;
use crate::subagents::Subagent;

/// The most agents listed under a session; with more, one last line says
/// how many are not listed.
pub const MAX_AGENT_LINES: usize = 4;

/// The badge before the count: plain ASCII, so it is one cell in every
/// font (doc/0029 explains why not `⑂`).
pub const BADGE_PREFIX: &str = "+";

/// Tree glyphs (box drawing, one cell each).
const TREE_MID: &str = "├─ ";
const TREE_END: &str = "└─ ";

/// Cells between the name column's start and the tree glyph.
const TREE_INDENT: usize = 1;

/// The fewest cells of description a child line needs; a narrower list
/// shows only the badges.
const MIN_LABEL: usize = 8;

/// The widest agent type shown.
const TYPE_MAX: usize = 24;

/// Width of the age column (doc/0006).
const AGE_W: usize = 4;

/// Gap between columns.
const GAP: usize = 2;

/// The count badge of `m`, e.g. `+3`, when it has running subagents.
pub fn badge(app: &App, m: &MergedSession) -> Option<String> {
    match app.subagents_of(&m.session).len() {
        0 => None,
        n => Some(format!("{BADGE_PREFIX}{n}")),
    }
}

/// The name column's width for a name of `name_w` cells (already capped)
/// and its badge: the badge and a gap after the name.
pub fn name_width(name_w: usize, badge: Option<&str>) -> usize {
    name_w + badge.map_or(0, |b| GAP + b.width())
}

/// The name cell, exactly `w` cells: the name, and the badge at the
/// cell's right end so badges line up; a narrow cell cuts the name first,
/// then shows only the badge.
pub fn name_cell(name: &str, badge: Option<&str>, w: usize) -> String {
    match badge {
        None => fit(name, w),
        Some(b) if w > b.width() + GAP => {
            format!("{}{}{b}", fit(name, w - b.width() - GAP), " ".repeat(GAP))
        }
        Some(b) => fit(b, w),
    }
}

/// One line under a session.
#[derive(Debug, Clone, Copy)]
pub enum Child<'a> {
    /// A running agent; `last` draws `└─` instead of `├─`.
    Agent { agent: &'a Subagent, last: bool },
    /// `… N more`: the agents beyond [`MAX_AGENT_LINES`].
    More(usize),
}

/// The lines under a session with `agents` running.
pub fn children(agents: &[Subagent]) -> Vec<Child<'_>> {
    let listed = agents.len().min(MAX_AGENT_LINES);
    let more = agents.len() - listed;
    let mut out: Vec<Child> = agents[..listed]
        .iter()
        .enumerate()
        .map(|(k, agent)| Child::Agent {
            agent,
            last: more == 0 && k + 1 == listed,
        })
        .collect();
    if more > 0 {
        out.push(Child::More(more));
    }
    out
}

/// The type column's width for `agents`: their widest type, at most
/// [`TYPE_MAX`] cells; 0 when none has one.
pub fn type_width<'a>(agents: impl IntoIterator<Item = &'a Subagent>) -> usize {
    agents
        .into_iter()
        .map(|a| a.agent_type.width())
        .max()
        .unwrap_or(0)
        .min(TYPE_MAX)
}

/// The type column's width for the whole list: the agents listed under
/// every session, folded or not, so the column does not move when a group
/// folds.
pub fn list_type_width(app: &App) -> usize {
    type_width(
        app.sessions()
            .iter()
            .flat_map(|m| app.subagents_of(&m.session).iter().take(MAX_AGENT_LINES)),
    )
}

/// Where a style's row columns sit, in cells from the line's start.
#[derive(Debug, Clone, Copy)]
pub struct Columns {
    /// The name column.
    pub name_at: usize,
    /// The age column.
    pub age_at: usize,
    /// The cells the line may use.
    pub width: usize,
    /// The type column's width ([`type_width`] of every agent shown).
    pub type_w: usize,
}

impl Columns {
    /// Where the age goes: the age column, or the line's right end when
    /// the age column is cut off (the minimal style does not fit columns
    /// to the width), so descriptions are cut first.
    fn age_start(&self) -> usize {
        (self.age_at + AGE_W).min(self.width).saturating_sub(AGE_W)
    }

    fn label_at(&self) -> usize {
        self.name_at + TREE_INDENT + TREE_MID.width()
    }

    /// The cells for the description and the type.
    fn room(&self) -> usize {
        self.age_start()
            .saturating_sub(GAP)
            .saturating_sub(self.label_at())
    }

    /// The type column's start, when it fits next to a description of at
    /// least [`MIN_LABEL`] cells.
    fn type_at(&self) -> Option<usize> {
        (self.type_w > 0 && self.room() >= MIN_LABEL + GAP + self.type_w)
            .then(|| self.age_start() - GAP - self.type_w)
    }

    /// The cells for the description.
    fn label_room(&self) -> usize {
        match self.type_at() {
            Some(at) => at - GAP - self.label_at(),
            None => self.room(),
        }
    }

    /// Whether child lines fit at all; below that only the badges show.
    pub fn fit_children(&self) -> bool {
        self.room() >= MIN_LABEL
    }
}

/// What a piece of a child line is, for the style to colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// Spaces.
    Pad,
    /// `├─`, `└─`.
    Tree,
    /// The description (or type, or id), or `… N more`.
    Label,
    /// The agent's type.
    Type,
    /// Time since the agent started, as the age column (doc/0006).
    Age,
}

/// The pieces of a child line, from the line's first cell: spaces up to
/// the tree glyph, the description, the type in a column of its own that
/// ends two cells before the age, and the age in the age column. A narrow
/// line cuts the description first, then drops the type column. Never
/// wider than `cols.width` while [`Columns::fit_children`].
pub fn child_line(child: &Child, cols: &Columns, now_ms: i64) -> Vec<(String, Part)> {
    let mut out = vec![
        (" ".repeat(cols.name_at + TREE_INDENT), Part::Pad),
        (
            match child {
                Child::Agent { last: false, .. } => TREE_MID,
                _ => TREE_END,
            }
            .to_owned(),
            Part::Tree,
        ),
    ];
    let label_at = cols.label_at();
    let room = cols.label_room();
    let agent = match child {
        Child::Agent { agent, .. } => *agent,
        Child::More(n) => {
            out.push((fit_short(&format!("… {n} more"), room), Part::Label));
            return out;
        }
    };
    let label = agent
        .description
        .as_deref()
        .filter(|d| !d.is_empty())
        .or_else(|| Some(agent.agent_type.as_str()).filter(|t| !t.is_empty()))
        .unwrap_or(&agent.id);
    let text = fit_short(label, room);
    let mut used = label_at + text.width();
    out.push((text, Part::Label));
    if let Some(at) = cols.type_at()
        && label != agent.agent_type
        && !agent.agent_type.is_empty()
    {
        out.push((" ".repeat(at - used), Part::Pad));
        let kind = fit_short(&agent.agent_type, cols.type_w);
        used = at + kind.width();
        out.push((kind, Part::Type));
    }
    let age_at = cols.age_start();
    if used + GAP <= age_at {
        let age = activity::age_label(now_ms.saturating_sub(agent.started_ms));
        out.push((" ".repeat(age_at - used), Part::Pad));
        out.push((rfit(&age, AGE_W), Part::Age));
    }
    out
}

/// `s` cut to at most `w` cells (with `…`), not padded.
fn fit_short(s: &str, w: usize) -> String {
    if s.width() <= w {
        s.to_owned()
    } else {
        fit(s, w).trim_end().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, ty: &str, desc: Option<&str>, started_ms: i64) -> Subagent {
        Subagent {
            id: id.into(),
            agent_type: ty.into(),
            description: desc.map(Into::into),
            started_ms,
        }
    }

    /// The cell where `needle` starts in `line`.
    fn col(line: &str, needle: &str) -> usize {
        line[..line.find(needle).expect(needle)].width()
    }

    fn text(parts: &[(String, Part)]) -> String {
        parts.iter().map(|(s, _)| s.as_str()).collect()
    }

    const COLS: Columns = Columns {
        name_at: 5,
        age_at: 60,
        width: 70,
        type_w: 15,
    };

    #[test]
    fn name_cells_put_the_badge_at_the_end() {
        assert_eq!(name_cell("alpha", None, 8), "alpha   ");
        assert_eq!(name_cell("alpha", Some("+3"), 10), "alpha   +3");
        assert_eq!(name_cell("alphabet-soup", Some("+3"), 10), "alpha…  +3");
        assert_eq!(name_cell("alpha", Some("+12"), 4), "+12 ");
        assert_eq!(name_cell("alpha", Some("+12"), 2), "+…");
        assert_eq!(name_width(10, Some("+3")), 14);
        assert_eq!(name_width(10, None), 10);
    }

    #[test]
    fn four_agents_are_listed_and_more_are_counted() {
        let agents: Vec<Subagent> = (0..6)
            .map(|k| agent(&format!("a{k}"), "general-purpose", None, k))
            .collect();
        let one = children(&agents[..1]);
        assert!(matches!(one[..], [Child::Agent { last: true, .. }]));
        let four = children(&agents[..4]);
        assert_eq!(four.len(), 4);
        assert!(matches!(four[3], Child::Agent { last: true, .. }));
        assert!(matches!(four[2], Child::Agent { last: false, .. }));
        let six = children(&agents);
        assert_eq!(six.len(), 5);
        assert!(matches!(six[3], Child::Agent { last: false, .. }));
        assert!(matches!(six[4], Child::More(2)));
    }

    #[test]
    fn child_lines_align_type_and_age_and_cut_the_description_first() {
        let types = [
            agent("a", "Explore", None, 0),
            agent("b", &"t".repeat(40), None, 0),
        ];
        assert_eq!(type_width(&types[..1]), 7);
        assert_eq!(type_width(&types), TYPE_MAX);
        assert_eq!(type_width(&[]), 0);
        let a = agent("a1", "Explore", Some("search docs"), 0);
        let line = text(&child_line(
            &Child::Agent {
                agent: &a,
                last: false,
            },
            &COLS,
            125_000,
        ));
        assert_eq!(col(&line, "├─ search docs"), 6);
        assert_eq!(col(&line, "Explore"), 43, "{line}");
        assert_eq!(&line[line.len() - 4..], "  2m");
        assert_eq!(line.width(), 64);

        let long = agent("a2", "general-purpose", Some(&"x".repeat(80)), 0);
        let line = text(&child_line(
            &Child::Agent {
                agent: &long,
                last: true,
            },
            &COLS,
            0,
        ));
        assert!(line.contains("└─ xxx"), "{line}");
        assert!(line.contains("…  general-purpose"), "{line}");
        assert!(line.ends_with(" now"), "{line}");
        assert_eq!(line.width(), 64);

        // Without a description the type is the label, shown once.
        let bare = agent("a3", "Explore", None, 0);
        let line = text(&child_line(
            &Child::Agent {
                agent: &bare,
                last: true,
            },
            &COLS,
            0,
        ));
        assert_eq!(line.matches("Explore").count(), 1, "{line}");
        let anon = agent("a4", "", None, 0);
        assert!(
            text(&child_line(
                &Child::Agent {
                    agent: &anon,
                    last: true
                },
                &COLS,
                0
            ))
            .contains("└─ a4")
        );
        let more = text(&child_line(&Child::More(2), &COLS, 0));
        assert_eq!(more.trim(), "└─ … 2 more");
    }

    #[test]
    fn narrow_lines_drop_the_type_then_stop_fitting() {
        let a = agent("a1", "general-purpose", Some("a long description"), 0);
        let cols = Columns {
            name_at: 2,
            age_at: 40,
            width: 30,
            type_w: 15,
        };
        assert!(cols.fit_children());
        let parts = child_line(
            &Child::Agent {
                agent: &a,
                last: true,
            },
            &cols,
            0,
        );
        let line = text(&parts);
        assert!(line.width() <= 30, "{line}");
        assert!(!parts.iter().any(|(_, p)| *p == Part::Type), "{line}");
        assert!(line.ends_with(" now"), "{line}");
        for width in 0..32 {
            let cols = Columns { width, ..cols };
            assert_eq!(cols.fit_children(), width >= 20, "{width}");
            let line = text(&child_line(
                &Child::Agent {
                    agent: &a,
                    last: true,
                },
                &cols,
                0,
            ));
            assert!(line.width() <= width.max(5 + MIN_LABEL), "{width}: {line}");
        }
    }
}
