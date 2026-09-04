//! What each seat is asked for, and what it must give back.

use std::sync::Arc;

use spoon_concept::Concept;

use crate::client::LlmError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Seat {
    Ears,
    Mouth,
    Teacher,
}

impl Seat {
    pub fn as_str(self) -> &'static str {
        match self {
            Seat::Ears => "ears",
            Seat::Mouth => "mouth",
            Seat::Teacher => "teacher",
        }
    }
}

/// A turn already taken, as context for reading the next one.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub said: Arc<str>,
    /// The concept it was read as, rendered.
    pub understood: Arc<str>,
}

/// What the ears produce from one utterance.
#[derive(Debug, Clone, PartialEq)]
pub struct Heard {
    /// The utterance as a sequence of concept operations.
    ///
    /// Flat rather than nested, because that is the shape speech actually has:
    /// bind, correct, qualify, qualify. Spoon reshapes it into a tree during
    /// resolution, where it has the context to do so.
    pub steps: Vec<Concept>,
    /// Words the ears could not place, in the position they appeared. Each one
    /// is a lead for the Teacher rather than a failure.
    pub unknown: Vec<Arc<str>>,
    /// How much the ears trust this reading, in `[0, 1]`.
    pub confidence: f64,
    /// Whether a model was consulted. The headline weaning metric is how often
    /// this is false.
    pub used_model: bool,
    /// Every name as it was written.
    ///
    /// A symbol id is computed from its name, so parsing a concept teaches the
    /// store nothing about spelling. Without carrying the words back, a brain
    /// prints `#8faadc4403462050` where it should print `owns`, and every reply
    /// about anything newly learned is unreadable.
    pub names: Vec<Arc<str>>,
}

impl Heard {
    pub fn native(steps: Vec<Concept>, confidence: f64) -> Self {
        Heard {
            steps,
            unknown: Vec::new(),
            confidence,
            used_model: false,
            names: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// Messy language in, concepts out.
#[async_trait::async_trait]
pub trait Ears: Send + Sync {
    /// Interpret an utterance.
    ///
    /// `vocabulary` is the concepts worth showing this turn, ranked by
    /// activation and already described: name, shape, and what each does.
    /// Passing everything Spoon knows would be slower and worse, since it
    /// buries the handful this user actually reaches for.
    ///
    /// Descriptions rather than bare names because a name alone leaves the
    /// model guessing at how a concept is called, and every wrong guess becomes
    /// a special case somewhere else in the system.
    ///
    /// `rules` are lessons the Teacher wrote after earlier misreadings. They
    /// are what makes the prompt something Spoon accumulates rather than a
    /// fixed string somebody maintains.
    ///
    /// `recent` is the last few turns, newest last, each as what was said and
    /// what it was read as. Without it every utterance is heard in isolation,
    /// and half of ordinary speech refers backwards: "that is called a
    /// palindrome" is not interpretable at all on its own, and the honest
    /// reading of it is a hole where the referent should be.
    async fn hear(
        &self,
        text: &str,
        vocabulary: &[String],
        recent: &[Turn],
        rules: &[String],
    ) -> Result<Heard, LlmError>;

    /// A reading produced without consulting a model, or `None` when the native
    /// path does not recognize the utterance. Always tried first.
    fn hear_native(&self, text: &str) -> Option<Heard>;
}

/// Structured result out, prose back.
#[async_trait::async_trait]
pub trait Mouth: Send + Sync {
    /// Render a response.
    ///
    /// `must_mention` holds the values that have to survive into the output.
    /// The check is what stops the mouth quietly inventing or dropping a number
    /// on its way to sounding natural.
    async fn say(&self, response: &Concept, must_mention: &[Concept]) -> Result<String, LlmError>;

    /// Deterministic rendering, used offline and whenever the model's output
    /// fails the faithfulness check.
    fn say_native(&self, response: &Concept, must_mention: &[Concept]) -> String;
}

/// What the Teacher is being asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum TeacherAsk {
    /// A word the ears could not place, with the utterance it appeared in.
    Vocabulary {
        word: Arc<str>,
        utterance: Arc<str>,
        position: Arc<str>,
    },
    /// A concept with no realization, with what was tried.
    Capability {
        concept: Concept,
        attempted: Vec<Arc<str>>,
    },
    /// Input and output examples for a capability, so the synthesizer has
    /// something to search against.
    Examples { concept: Concept, arity: usize },
    /// What a word means, when Spoon has no concept for it at all.
    Concept { word: Arc<str>, context: Arc<str> },
    /// Whether an utterance was read correctly, and what it should have been.
    ///
    /// The question the Teacher is best at. When a turn goes wrong the cause is
    /// often the reading rather than a missing capability, and judging whether
    /// an interpretation matches what someone said is a far easier job than
    /// writing a body that satisfies examples.
    ///
    /// It is also the only ask whose answer compounds. A corrected reading is
    /// stored as a phrasing, so the native path handles that shape from then on
    /// without a model at all. Everything else the Teacher does helps once.
    Reading {
        utterance: Arc<str>,
        /// How it was read, rendered.
        heard: Arc<str>,
        /// What went wrong with acting on it.
        trouble: Arc<str>,
    },
}

/// A specification the synthesizer can search against.
///
/// The Teacher writes these. It does not write executable bodies: proposing
/// structure is a different act from writing code that runs, and the
/// synthesizer verifies what it builds against these examples.
#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub target: Concept,
    pub examples: Vec<(Vec<Concept>, Concept)>,
    pub note: Option<Arc<str>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TeacherReply {
    /// The word means an existing concept.
    Synonym {
        word: Arc<str>,
        concept: Concept,
        confidence: f64,
    },
    /// A new concept, with the relationships that give it consequences.
    NewConcept {
        concept: Concept,
        relations: Vec<Concept>,
        surface_forms: Vec<Arc<str>>,
    },
    /// Examples for the synthesizer.
    Spec(Spec),
    /// A realization built from concepts Spoon already has.
    Composition { target: Concept, body: Concept },
    /// A corrected reading of the utterance, as concept steps.
    ///
    /// Kept as a phrasing rather than only applied to this turn, because the
    /// same shape will be said again and the point is to stop paying for it.
    Reading {
        steps: Vec<Concept>,
        /// A rule worth remembering, when the mistake was one of a kind rather
        /// than one of a shape.
        ///
        /// A phrasing fixes the sentence that was said. A rule fixes every
        /// sentence that would have gone the same way: writing a list as
        /// `list<[a, b, c]>` is not a fact about that utterance, it is a
        /// misunderstanding of the notation, and stating it once is worth more
        /// than correcting it a hundred times.
        lesson: Option<Arc<str>>,
    },
    /// The Teacher had nothing useful, which is a real answer and not a
    /// failure. Recording it stops Spoon asking the same question forever.
    Unknown { why: Arc<str> },
}

/// Fills gaps, and never writes executable bodies by default.
#[async_trait::async_trait]
pub trait Teacher: Send + Sync {
    /// Answer a question about something Spoon could not do.
    ///
    /// `vocabulary` is what Spoon currently knows, ranked. Without it the
    /// Teacher is guessing at what it may compose from and will refuse work it
    /// could have done: asked to build string reversal it will say no concept
    /// turns a string into a list, while `chars` sits in the store unmentioned.
    async fn teach(
        &self,
        ask: &TeacherAsk,
        vocabulary: &[Arc<str>],
    ) -> Result<TeacherReply, LlmError>;
}
