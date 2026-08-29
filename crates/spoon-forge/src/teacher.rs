use spoon_engine::{TeacherProposalWire, TeacherRequestWire};

use crate::ForgeError;

/// The Teacher seam for a curriculum run.
///
/// The runner never reaches a provider itself. A host wires a real Teacher in
/// here; tests wire a deterministic script. Returning an error aborts the
/// cycle rather than silently abstaining, so a provider outage is
/// distinguishable from a Teacher that had nothing to offer.
pub trait CurriculumTeacher {
    fn respond(&mut self, request: &TeacherRequestWire) -> Result<TeacherProposalWire, ForgeError>;
}

/// Counts Teacher turns across a run so a teacher-off phase can prove absence
/// rather than assert it.
pub(crate) struct TeacherSession<'a> {
    teacher: &'a mut dyn CurriculumTeacher,
    calls: u32,
}

impl<'a> TeacherSession<'a> {
    pub(crate) fn new(teacher: &'a mut dyn CurriculumTeacher) -> Self {
        Self { teacher, calls: 0 }
    }

    pub(crate) fn calls(&self) -> u32 {
        self.calls
    }

    pub(crate) fn respond(
        &mut self,
        request: &TeacherRequestWire,
    ) -> Result<TeacherProposalWire, ForgeError> {
        self.calls += 1;
        self.teacher.respond(request)
    }
}
