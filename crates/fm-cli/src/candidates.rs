//! The models compared in T2.4 (SPEC decision 33), each pinned to one exact GGUF file:
//! a Hugging Face repository at a fixed revision, and the file's size and SHA-256.

use std::borrow::Cow;

use fm_llm::{ModelSpec, sha256_hex};

/// One candidate: its spec and what the report says about it.
pub struct Candidate {
    /// Short name used on the command line and in results.
    pub name: &'static str,
    pub spec: ModelSpec,
    pub licence: &'static str,
    /// Who made the weights, and who made the GGUF.
    pub provenance: &'static str,
    /// Parameters, for the report.
    pub parameters: &'static str,
    /// Put before the answer in the assistant's turn: thinking models get an empty
    /// thinking block, as their chat template renders "thinking off".
    pub answer_lead: &'static str,
}

const fn hf(repo_rev_file: &'static str) -> Cow<'static, str> {
    Cow::Borrowed(repo_rev_file)
}

pub const CANDIDATES: &[Candidate] = &[
    Candidate {
        name: "qwen3.5-4b",
        spec: ModelSpec {
            id: Cow::Borrowed("qwen3.5-4b-q4_k_m"),
            url: hf(
                "https://huggingface.co/unsloth/Qwen3.5-4B-GGUF/resolve/e87f176479d0855a907a41277aca2f8ee7a09523/Qwen3.5-4B-Q4_K_M.gguf",
            ),
            file_name: Cow::Borrowed("Qwen3.5-4B-Q4_K_M.gguf"),
            size: 2_740_937_888,
            sha256: sha256_hex("00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4"),
        },
        licence: "Apache-2.0",
        provenance: "Qwen (Alibaba), 2026-03; GGUF by unsloth",
        parameters: "4.7B",
        answer_lead: "<think>\n\n</think>\n\n",
    },
    Candidate {
        name: "qwen3-4b-2507",
        spec: ModelSpec {
            id: Cow::Borrowed("qwen3-4b-instruct-2507-q4_k_m"),
            url: hf(
                "https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/a06e946bb6b655725eafa393f4a9745d460374c9/Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
            ),
            file_name: Cow::Borrowed("Qwen3-4B-Instruct-2507-Q4_K_M.gguf"),
            size: 2_497_281_120,
            sha256: sha256_hex("3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597"),
        },
        licence: "Apache-2.0",
        provenance: "Qwen (Alibaba), 2025-08; GGUF by unsloth",
        parameters: "4.0B",
        answer_lead: "",
    },
    Candidate {
        name: "ministral-3-3b",
        spec: ModelSpec {
            id: Cow::Borrowed("ministral-3-3b-instruct-2512-q4_k_m"),
            url: hf(
                "https://huggingface.co/mistralai/Ministral-3-3B-Instruct-2512-GGUF/resolve/eb599d408350ea2bb60452cb86be7c7b2fc28227/Ministral-3-3B-Instruct-2512-Q4_K_M.gguf",
            ),
            file_name: Cow::Borrowed("Ministral-3-3B-Instruct-2512-Q4_K_M.gguf"),
            size: 2_147_023_008,
            sha256: sha256_hex("9ed150d4367e68df0ac8e1540f6ddc65b42d0ee26378329d1ecbca60f93fc5f8"),
        },
        licence: "Apache-2.0",
        provenance: "Mistral AI, 2025-12; official GGUF",
        parameters: "3.8B",
        answer_lead: "",
    },
    Candidate {
        name: "phi-4-mini",
        spec: ModelSpec {
            id: Cow::Borrowed("phi-4-mini-instruct-q4_k_m"),
            url: hf(
                "https://huggingface.co/unsloth/Phi-4-mini-instruct-GGUF/resolve/78eb92a46fc37e6b524df991ed9aca9bc6aa7b80/Phi-4-mini-instruct-Q4_K_M.gguf",
            ),
            file_name: Cow::Borrowed("Phi-4-mini-instruct-Q4_K_M.gguf"),
            size: 2_491_874_272,
            sha256: sha256_hex("88c00229914083cd112853aab84ed51b87bdf6b9ce42f532d8c85c7c63b1730a"),
        },
        licence: "MIT",
        provenance: "Microsoft, 2025-02; GGUF by unsloth",
        parameters: "3.8B",
        answer_lead: "",
    },
    Candidate {
        name: "granite-4.0-micro",
        spec: ModelSpec {
            id: Cow::Borrowed("granite-4.0-micro-q4_k_m"),
            url: hf(
                "https://huggingface.co/ibm-granite/granite-4.0-micro-GGUF/resolve/ec48475f0c811d812fbfb61975717a9c36eeb652/granite-4.0-micro-Q4_K_M.gguf",
            ),
            file_name: Cow::Borrowed("granite-4.0-micro-Q4_K_M.gguf"),
            size: 2_099_502_528,
            sha256: sha256_hex("97c417dcc0534b0737c74016fb2af083cb17c3b51eaac621192d23961b7024eb"),
        },
        licence: "Apache-2.0",
        provenance: "IBM, 2025-10 (reserve); official GGUF",
        parameters: "3.4B",
        answer_lead: "",
    },
];

/// The four compared by default; the last entry is the reserve.
pub fn main_candidates() -> &'static [Candidate] {
    &CANDIDATES[..4]
}

pub fn by_name(name: &str) -> Option<&'static Candidate> {
    CANDIDATES.iter().find(|c| c.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_candidate_is_pinned_to_a_revision_on_an_allowed_origin() {
        for c in CANDIDATES {
            let url = c.spec.url.as_ref();
            assert!(url.starts_with("https://huggingface.co/"), "{}", c.name);
            let rev = url
                .split("/resolve/")
                .nth(1)
                .unwrap()
                .split('/')
                .next()
                .unwrap();
            assert_eq!(rev.len(), 40, "{}: revision is a full commit hash", c.name);
            assert!(url.ends_with(c.spec.file_name.as_ref()), "{}", c.name);
            assert!(c.spec.size > 1_000_000_000);
        }
        let mut names: Vec<_> = CANDIDATES.iter().map(|c| c.name).collect();
        names.dedup();
        assert_eq!(names.len(), CANDIDATES.len());
    }
}
