use serde::Serialize;
use tokio::sync::mpsc;

use crate::episode::{EarsPath, MouthPath};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum TurnEvent {
    TurnStarted {
        session: String,
        text: String,
    },
    CorrectionApplied {
        previous_episode: u64,
        retracted: usize,
    },
    EarsStarted,
    EarsResult {
        path: EarsPath,
        steps: Vec<String>,
        unknown: Vec<String>,
    },
    Reconciled {
        renamed: usize,
    },
    InteriorStarted {
        goal: Option<String>,
    },
    ActStep {
        concept: String,
        realization: Option<String>,
        outcome: String,
        depth: u32,
    },
    GapFound {
        concept: String,
    },
    InteriorResult {
        result: Option<String>,
    },
    TeacherConsulted {
        ask_kind: String,
        subject: Option<String>,
    },
    TeacherReply {
        kind: String,
        summary: String,
    },
    Learned {
        note: String,
    },
    MouthStarted,
    MouthResult {
        path: MouthPath,
        reply: String,
    },
    TurnFinished {
        episode_id: u64,
        millis_total: u64,
    },
}

impl TurnEvent {
    pub fn event_name(&self) -> &'static str {
        match self {
            TurnEvent::TurnStarted { .. } => "turn-started",
            TurnEvent::CorrectionApplied { .. } => "correction-applied",
            TurnEvent::EarsStarted => "ears-started",
            TurnEvent::EarsResult { .. } => "ears-result",
            TurnEvent::Reconciled { .. } => "reconciled",
            TurnEvent::InteriorStarted { .. } => "interior-started",
            TurnEvent::ActStep { .. } => "act-step",
            TurnEvent::GapFound { .. } => "gap-found",
            TurnEvent::InteriorResult { .. } => "interior-result",
            TurnEvent::TeacherConsulted { .. } => "teacher-consulted",
            TurnEvent::TeacherReply { .. } => "teacher-reply",
            TurnEvent::Learned { .. } => "learned",
            TurnEvent::MouthStarted => "mouth-started",
            TurnEvent::MouthResult { .. } => "mouth-result",
            TurnEvent::TurnFinished { .. } => "turn-finished",
        }
    }
}

#[derive(Clone)]
pub struct EventSink(mpsc::UnboundedSender<TurnEvent>);

impl EventSink {
    pub fn new(sender: mpsc::UnboundedSender<TurnEvent>) -> Self {
        EventSink(sender)
    }

    pub fn send(&self, event: TurnEvent) {
        let _ = self.0.send(event);
    }
}

pub fn emit(sink: Option<&EventSink>, event: TurnEvent) {
    if let Some(s) = sink {
        s.send(event);
    }
}
