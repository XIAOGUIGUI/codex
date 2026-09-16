use super::ContextualUserFragment;
use codex_protocol::models::ContentItemKind;
use codex_protocol::protocol::MAX_COMPACTION_GUIDANCE_BYTES;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompactionGuidance {
    guidance: String,
}

impl CompactionGuidance {
    pub(crate) fn parse(guidance: Option<String>) -> Result<Option<Self>, String> {
        let Some(guidance) = guidance else {
            return Ok(None);
        };
        let guidance = guidance.trim();
        if guidance.is_empty() {
            return Ok(None);
        }
        if guidance.len() > MAX_COMPACTION_GUIDANCE_BYTES {
            return Err(format!(
                "compaction guidance exceeds the {MAX_COMPACTION_GUIDANCE_BYTES}-byte limit"
            ));
        }
        Ok(Some(Self {
            guidance: guidance.to_string(),
        }))
    }

    pub(crate) fn append_to(&self, prompt: &mut String) {
        prompt.push_str("\n\n");
        prompt.push_str(&self.render());
    }
}

impl ContextualUserFragment for CompactionGuidance {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("compaction.guidance".to_string())
    }

    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<compaction_guidance>\n", "\n</compaction_guidance>")
    }

    fn body(&self) -> String {
        self.guidance.clone()
    }
}

#[cfg(test)]
#[path = "compaction_guidance_tests.rs"]
mod tests;
