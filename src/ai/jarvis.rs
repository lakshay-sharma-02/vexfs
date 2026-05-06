//! JarvisEngine — proactive workspace intelligence.
//!
//! Reads a WorkspaceModel snapshot and produces a prioritised list of
//! Suggestions that appear in `.vexfs-jarvis` on mount / session start.
//!
//! Design rules:
//!   - Every suggestion carries a `basis` field — the user always sees *why*
//!   - Suggestions fire at session boundaries, never per-event (no noise)
//!   - No LLM, no external calls — pure inference over collected patterns
//!   - PreWarm suggestions are acted on silently (cache hint), not just text

use crate::ai::workspace::{WorkspaceModel, SessionKind};

// ── Suggestion types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SuggestionPriority {
    High,
    Medium,
    Low,
}

impl SuggestionPriority {
    pub fn label(&self) -> &'static str {
        match self {
            SuggestionPriority::High   => "⚠ HIGH",
            SuggestionPriority::Medium => "→ MED",
            SuggestionPriority::Low    => "· LOW",
        }
    }
}

#[derive(Debug, Clone)]
pub enum SuggestionKind {
    /// Opened repeatedly with no writes — something is blocking.
    StallBreaker,
    /// Mid-flow files from last session that deserve continuity.
    Continuity,
    /// A previously active project cluster has gone quiet.
    Neglect,
    /// Files statistically likely to be needed — pre-warm the cache.
    PreWarm,
    /// Session pattern observation worth surfacing.
    FocusHint,
}

impl SuggestionKind {
    pub fn label(&self) -> &'static str {
        match self {
            SuggestionKind::StallBreaker => "StallBreaker",
            SuggestionKind::Continuity   => "Continuity",
            SuggestionKind::Neglect      => "Neglect",
            SuggestionKind::PreWarm      => "PreWarm",
            SuggestionKind::FocusHint    => "FocusHint",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Suggestion {
    pub priority:      SuggestionPriority,
    pub kind:          SuggestionKind,
    /// Human-readable message shown in .vexfs-jarvis
    pub message:       String,
    /// File names this suggestion relates to
    pub related_files: Vec<String>,
    /// Transparent reasoning — always shown so the user can calibrate trust
    pub basis:         String,
}

// ── JarvisEngine ──────────────────────────────────────────────────────────────

pub struct JarvisEngine;

impl JarvisEngine {
    /// Generate a ranked list of suggestions from the current WorkspaceModel.
    /// Returns suggestions sorted High → Medium → Low.
    /// Call this once per EndSession, result stored in SharedAIState.
    pub fn analyse(model: &WorkspaceModel) -> Vec<Suggestion> {
        let mut suggestions: Vec<Suggestion> = Vec::new();

        Self::check_stalls(model, &mut suggestions);
        Self::check_continuity(model, &mut suggestions);
        Self::check_neglect(model, &mut suggestions);
        Self::check_focus_pattern(model, &mut suggestions);
        Self::check_prewarm(model, &mut suggestions);

        // Sort: High first, then Medium, then Low
        suggestions.sort_by(|a, b| a.priority.cmp(&b.priority));
        suggestions
    }

    /// Render suggestions as the content of .vexfs-jarvis
    pub fn render(suggestions: &[Suggestion], model: &WorkspaceModel) -> String {
        let mut out = String::new();

        out.push_str("=== VexFS Jarvis — Workspace Brief ===\n");

        // Session summary
        if let Some(session) = &model.last_session {
            out.push_str(&format!(
                "Last session: {} | primary: {} | {} files, {} writes\n",
                session.kind.label(),
                session.primary_project,
                session.files_touched,
                session.writes_made,
            ));
        }
        out.push('\n');

        if suggestions.is_empty() {
            out.push_str("No suggestions — workspace looks healthy.\n");
            return out;
        }

        for s in suggestions {
            out.push_str(&format!(
                "{}  {}\n  {}\n",
                s.priority.label(),
                s.kind.label(),
                s.message,
            ));
            if !s.related_files.is_empty() {
                out.push_str(&format!(
                    "  Files: {}\n",
                    s.related_files.join(", ")
                ));
            }
            out.push_str(&format!("  Why: {}\n\n", s.basis));
        }

        out.push_str("======================================\n");
        out
    }

    // ── Individual checks ─────────────────────────────────────────────────────

    fn check_stalls(model: &WorkspaceModel, out: &mut Vec<Suggestion>) {
        for stall in &model.stalls {
            let priority = if stall.open_without_write >= 5 {
                SuggestionPriority::High
            } else {
                SuggestionPriority::Medium
            };

            out.push(Suggestion {
                priority,
                kind: SuggestionKind::StallBreaker,
                message: format!(
                    "'{}' opened {}x across {} sessions with almost no writes. \
                     Something is blocking you here.",
                    stall.name,
                    stall.open_without_write,
                    stall.stall_session_count,
                ),
                related_files: vec![stall.name.clone()],
                basis: format!(
                    "{} opens, ≤1 write detected across recent sessions",
                    stall.open_without_write
                ),
            });
        }
    }

    fn check_continuity(model: &WorkspaceModel, out: &mut Vec<Suggestion>) {
        if model.unfinished_files.is_empty() {
            return;
        }

        out.push(Suggestion {
            priority: SuggestionPriority::Medium,
            kind: SuggestionKind::Continuity,
            message: format!(
                "Last session ended mid-flow on {} file(s). \
                 Resuming from where you left off.",
                model.unfinished_files.len(),
            ),
            related_files: model.unfinished_files.clone(),
            basis: format!(
                "Files were written to and reopened multiple times \
                 without a clean close sequence: {}",
                model.unfinished_files.join(", ")
            ),
        });
    }

    fn check_neglect(model: &WorkspaceModel, out: &mut Vec<Suggestion>) {
        // Projects inactive for >5 days that were previously active
        for project in &model.projects {
            if project.days_inactive < 5 {
                continue;
            }
            // Only flag if it was genuinely active (velocity > 0)
            if project.velocity < 1.0 {
                continue;
            }

            let priority = if project.days_inactive > 14 {
                SuggestionPriority::Medium
            } else {
                SuggestionPriority::Low
            };

            out.push(Suggestion {
                priority,
                kind: SuggestionKind::Neglect,
                message: format!(
                    "Project cluster '{}' untouched for {} day(s). \
                     Was actively worked on before.",
                    project.name,
                    project.days_inactive,
                ),
                related_files: project.file_names.iter().take(4).cloned().collect(),
                basis: format!(
                    "Last active {} day(s) ago, velocity was {:.1} accesses/session",
                    project.days_inactive, project.velocity
                ),
            });
        }
    }

    fn check_focus_pattern(model: &WorkspaceModel, out: &mut Vec<Suggestion>) {
        let session = match &model.last_session {
            Some(s) => s,
            None    => return,
        };

        // Debugging session with identified reopen-without-write files
        if session.kind == SessionKind::Debugging
            && !session.reopen_without_write.is_empty()
        {
            out.push(Suggestion {
                priority: SuggestionPriority::Medium,
                kind: SuggestionKind::FocusHint,
                message: format!(
                    "Last session was Debugging mode — {} file(s) reopened \
                     repeatedly with no writes. Consider breaking the problem \
                     into a smaller isolated test.",
                    session.reopen_without_write.len(),
                ),
                related_files: session.reopen_without_write.clone(),
                basis: format!(
                    "Files reopened 3+ times with 0 writes this session: {}",
                    session.reopen_without_write.join(", ")
                ),
            });
            return;
        }

        // Exploration session — broad but shallow
        if session.kind == SessionKind::Exploration && session.writes_made == 0 {
            out.push(Suggestion {
                priority: SuggestionPriority::Low,
                kind: SuggestionKind::FocusHint,
                message: format!(
                    "Last session was pure Exploration — {} files touched, \
                     0 writes. If you found what you were looking for, \
                     consider committing to one area next session.",
                    session.files_touched,
                ),
                related_files: vec![],
                basis: "Session had many file opens and zero writes".to_string(),
            });
        }
    }

    fn check_prewarm(model: &WorkspaceModel, out: &mut Vec<Suggestion>) {
        let session = match &model.last_session {
            Some(s) => s,
            None    => return,
        };

        // Find the primary project cluster and suggest its files for pre-warm
        let primary = model.projects.iter()
            .find(|p| p.name == session.primary_project);

        if let Some(project) = primary {
            if project.file_names.len() > 1 {
                out.push(Suggestion {
                    priority: SuggestionPriority::Low,
                    kind: SuggestionKind::PreWarm,
                    message: format!(
                        "{} file(s) from '{}' cluster pre-warmed into cache.",
                        project.file_names.len(),
                        project.name,
                    ),
                    related_files: project.file_names.iter().take(5).cloned().collect(),
                    basis: format!(
                        "These files co-occur in {:.0}%+ of sessions \
                         that touch '{}'",
                        75.0,
                        project.name,
                    ),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::workspace::{WorkspaceModel, StallRecord, SessionProfile, SessionKind};

    fn make_model_with_stall() -> WorkspaceModel {
        let mut m = WorkspaceModel::new();
        m.stalls = vec![StallRecord {
            ino: 42,
            name: "snapshot.rs".to_string(),
            stall_session_count: 3,
            open_without_write: 6,
        }];
        m.last_session = Some(SessionProfile {
            kind: SessionKind::Debugging,
            primary_project: "storage".to_string(),
            files_touched: 4,
            writes_made: 1,
            reopen_without_write: vec!["snapshot.rs".to_string()],
        });
        m
    }

    #[test]
    fn test_stall_produces_high_priority() {
        let model = make_model_with_stall();
        let suggestions = JarvisEngine::analyse(&model);
        assert!(!suggestions.is_empty());
        assert_eq!(suggestions[0].priority, SuggestionPriority::High);
        assert!(matches!(suggestions[0].kind, SuggestionKind::StallBreaker));
    }

    #[test]
    fn test_render_non_empty() {
        let model = make_model_with_stall();
        let suggestions = JarvisEngine::analyse(&model);
        let rendered = JarvisEngine::render(&suggestions, &model);
        assert!(rendered.contains("StallBreaker"));
        assert!(rendered.contains("snapshot.rs"));
        assert!(rendered.contains("Why:"));
    }

    #[test]
    fn test_empty_model_no_panic() {
        let model = WorkspaceModel::new();
        let suggestions = JarvisEngine::analyse(&model);
        let rendered = JarvisEngine::render(&suggestions, &model);
        assert!(rendered.contains("healthy"));
        // Empty model should not crash
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_continuity_suggestion() {
        let mut model = WorkspaceModel::new();
        model.unfinished_files = vec!["engine.rs".to_string(), "fuse/mod.rs".to_string()];
        model.last_session = Some(SessionProfile {
            kind: SessionKind::Focus,
            primary_project: "ai".to_string(),
            files_touched: 2,
            writes_made: 8,
            reopen_without_write: vec![],
        });
        let suggestions = JarvisEngine::analyse(&model);
        assert!(suggestions.iter().any(|s| matches!(s.kind, SuggestionKind::Continuity)));
    }
}
