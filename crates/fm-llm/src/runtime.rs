//! Loading a model and generating grammar-constrained output with llama.cpp.

use std::fmt;
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::{Mutex, OnceLock, PoisonError};

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{LlamaChatMessage, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;

use crate::download::{DownloadError, ModelSpec, verify_file};
use crate::grammar::{CHOICE_PREFIX, Choices};

/// Confidence of a certain answer, in basis points.
pub const MAX_BASIS_POINTS: u16 = 10_000;

/// Why a model couldn't be loaded or run.
#[derive(Debug)]
pub enum LlmError {
    /// The model file is missing or isn't the one the spec names.
    File(DownloadError),
    /// llama.cpp failed (backend, loading, context, decoding, grammar).
    Runtime(String),
    /// The prompt plus the answer doesn't fit the model's context window.
    PromptTooLong { tokens: usize, limit: u32 },
    /// The answer wasn't complete after `max_tokens` tokens.
    TooManyTokens { max_tokens: u32 },
    /// The grammar left no token to pick. A bug in the grammar, never the
    /// model's doing.
    NoAllowedToken,
    /// The model's tokenizer can't spell this key's answer exactly.
    Tokenization { key: String },
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(e) => write!(f, "{e}"),
            Self::Runtime(why) => write!(f, "llama.cpp error: {why}"),
            Self::PromptTooLong { tokens, limit } => {
                write!(
                    f,
                    "prompt needs {tokens} tokens; the model's limit is {limit}"
                )
            }
            Self::TooManyTokens { max_tokens } => {
                write!(f, "answer not complete after {max_tokens} tokens")
            }
            Self::NoAllowedToken => f.write_str("the grammar allows no next token"),
            Self::Tokenization { key } => {
                write!(f, "the model can't spell category key {key:?} exactly")
            }
        }
    }
}

impl std::error::Error for LlmError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::File(e) => Some(e),
            _ => None,
        }
    }
}

impl From<DownloadError> for LlmError {
    fn from(e: DownloadError) -> Self {
        Self::File(e)
    }
}

fn runtime_error(e: impl fmt::Display) -> LlmError {
    LlmError::Runtime(e.to_string())
}

/// llama.cpp's backend can be initialised once per process.
fn backend() -> Result<&'static LlamaBackend, LlmError> {
    static BACKEND: OnceLock<Result<LlamaBackend, String>> = OnceLock::new();
    BACKEND
        .get_or_init(|| {
            let mut backend = LlamaBackend::init().map_err(|e| e.to_string())?;
            // llama.cpp logs to stderr, and some of its debug paths print
            // prompt text. Prompts hold financial data, so keep it quiet.
            backend.void_logs();
            Ok(backend)
        })
        .as_ref()
        .map_err(|e| LlmError::Runtime(e.clone()))
}

/// Whether llama.cpp was built with a GPU backend (Metal on Apple Silicon).
pub fn gpu_offload_supported() -> Result<bool, LlmError> {
    Ok(backend()?.supports_gpu_offload())
}

/// How the next token is picked among those the grammar allows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sampling {
    /// The most likely token (lowest id on a tie), so the same prompt always
    /// gives the same answer. What the app uses.
    Greedy,
    /// Random draws from the distribution at `temperature`, reproducible from
    /// `seed`. For tests that explore unlikely paths through the grammar.
    Random { seed: u64, temperature: f32 },
}

/// Settings for one generation.
#[derive(Debug, Clone, PartialEq)]
pub struct GenerateOptions {
    pub sampling: Sampling,
    /// Upper bound on generated tokens; reaching it is an error.
    pub max_tokens: u32,
    /// CPU threads; `None` keeps llama.cpp's default.
    pub threads: Option<u32>,
}

impl Default for GenerateOptions {
    fn default() -> Self {
        Self {
            sampling: Sampling::Greedy,
            max_tokens: 64,
            threads: None,
        }
    }
}

/// Text generated under a grammar, with how sure the model was of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    /// What the model generated after the prompt and prefix.
    pub text: String,
    /// See the crate docs: the product of the chosen tokens' probabilities
    /// within what the grammar allowed, in basis points.
    pub confidence_bp: u16,
    pub tokens: u32,
}

/// A key picked from a [`Choices`] list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// One of the list's keys, possibly [`crate::UNCATEGORISED`].
    pub key: String,
    pub confidence_bp: u16,
}

/// A loaded, verified model file.
pub struct Model {
    inner: LlamaModel,
    id: String,
    sha256: [u8; 32],
    /// Each token's bytes, indexed by token id.
    pieces: Vec<Vec<u8>>,
    /// llama.cpp contexts must not be created concurrently from one model.
    new_context: Mutex<()>,
}

impl fmt::Debug for Model {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Model")
            .field("id", &self.id)
            .field("sha256", &crate::download::hex(&self.sha256))
            .finish_non_exhaustive()
    }
}

impl Model {
    /// Checks the file at `path` against `spec` (size and SHA-256), then
    /// loads it. All layers go to the GPU when there is one.
    pub fn load(path: &Path, spec: &ModelSpec) -> Result<Self, LlmError> {
        verify_file(path, spec)?;
        let backend = backend()?;
        let inner = LlamaModel::load_from_file(backend, path, &LlamaModelParams::default())
            .map_err(runtime_error)?;
        let vocab = inner.vocab();
        let pieces = (0..inner.n_vocab())
            .map(|id| vocab.token_to_piece(LlamaToken(id), false, None))
            .collect();
        Ok(Self {
            inner,
            id: spec.id.to_string(),
            sha256: spec.sha256,
            pieces,
            new_context: Mutex::new(()),
        })
    }

    /// The spec's model id, for decision provenance.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// SHA-256 of the GGUF file that was loaded, for decision provenance.
    pub fn gguf_sha256(&self) -> [u8; 32] {
        self.sha256
    }

    /// A prompt in the model's own chat format, ending where the assistant's
    /// answer starts.
    pub fn chat_prompt(&self, system: &str, user: &str) -> Result<String, LlmError> {
        let template = self.inner.chat_template(None).map_err(runtime_error)?;
        let messages = [("system", system), ("user", user)]
            .into_iter()
            .map(|(role, content)| LlamaChatMessage::new(role.into(), content.into()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(runtime_error)?;
        self.inner
            .apply_chat_template(&template, &messages, true)
            .map_err(runtime_error)
    }

    /// Picks one key from `choices` (or [`crate::UNCATEGORISED`]). `prompt`
    /// is a full prompt, e.g. from [`Model::chat_prompt`]; the answer's
    /// opening `{"category":"` is appended to it.
    ///
    /// Two masks apply at every step: the choices' grammar, and the tokens
    /// that continue some key's canonical tokenisation (how the model's
    /// tokenizer spells that answer). The second keeps other spellings of the
    /// same text from taking probability, so confidences over the keys sum to
    /// 1 and a key that is the only option has confidence 10 000.
    pub fn choose(
        &self,
        prompt: &str,
        choices: &Choices,
        options: &GenerateOptions,
    ) -> Result<Choice, LlmError> {
        let mut trie = TokenTrie::default();
        for key in choices.keys() {
            trie.insert(&self.answer_tokens(key, &Choices::answer(key))?);
        }
        let generation = self.run(
            prompt,
            CHOICE_PREFIX,
            &choices.grammar(),
            Stop::Trie(&trie),
            options,
        )?;
        let key = choices
            .parse(&generation.text)
            .ok_or(LlmError::NoAllowedToken)?;
        Ok(Choice {
            key: key.to_string(),
            confidence_bp: generation.confidence_bp,
        })
    }

    /// Generates text after `prompt` + `prefix` that `grammar` (GBNF, root
    /// rule `root`) allows, stopping as soon as `is_complete` accepts it.
    ///
    /// Only the grammar masks tokens here, so the same text spelled with
    /// different tokens counts as different paths and the confidence can be
    /// below the text's total probability (see the crate docs).
    pub fn generate(
        &self,
        prompt: &str,
        prefix: &str,
        grammar: &str,
        is_complete: &dyn Fn(&str) -> bool,
        options: &GenerateOptions,
    ) -> Result<Generation, LlmError> {
        self.run(prompt, prefix, grammar, Stop::When(is_complete), options)
    }

    /// The canonical tokens of `answer` as it follows [`CHOICE_PREFIX`]. The
    /// answer is tokenised together with the prefix, as the model would see
    /// it, and the prefix's tokens dropped; if a token straddles the two, the
    /// answer is tokenised alone. Either way the tokens must spell exactly
    /// `answer`.
    fn answer_tokens(&self, key: &str, answer: &str) -> Result<Vec<LlamaToken>, LlmError> {
        let vocab = self.inner.vocab();
        let piece = |token: &LlamaToken| self.piece(*token);
        let spells = |tokens: &[LlamaToken]| {
            !tokens.is_empty() && tokens.iter().flat_map(piece).copied().eq(answer.bytes())
        };

        let joint = vocab.tokenize(format!("{CHOICE_PREFIX}{answer}").as_bytes(), false, false);
        let total: usize = joint.iter().map(|t| piece(t).len()).sum();
        // Some tokenizers add a leading space; the prefix then ends that much later.
        if let Some(lead) = total.checked_sub(CHOICE_PREFIX.len() + answer.len()) {
            let prefix_end = lead + CHOICE_PREFIX.len();
            let mut consumed = 0;
            for (i, token) in joint.iter().enumerate() {
                if consumed == prefix_end {
                    if spells(&joint[i..]) {
                        return Ok(joint[i..].to_vec());
                    }
                    break;
                }
                consumed += piece(token).len();
            }
        }
        let alone = vocab.tokenize(answer.as_bytes(), false, false);
        if spells(&alone) {
            return Ok(alone);
        }
        Err(LlmError::Tokenization {
            key: key.to_string(),
        })
    }

    fn piece(&self, token: LlamaToken) -> &[u8] {
        usize::try_from(token.0)
            .ok()
            .and_then(|i| self.pieces.get(i))
            .map_or(&[], Vec::as_slice)
    }

    fn run(
        &self,
        prompt: &str,
        prefix: &str,
        grammar: &str,
        stop: Stop<'_>,
        options: &GenerateOptions,
    ) -> Result<Generation, LlmError> {
        let backend = backend()?;
        let vocab = self.inner.vocab();
        let prompt_tokens = vocab.tokenize(format!("{prompt}{prefix}").as_bytes(), true, true);

        let limit = self.inner.n_ctx_train();
        let needed = prompt_tokens.len() + options.max_tokens as usize;
        let n_ctx = u32::try_from(needed)
            .ok()
            .filter(|&n| n <= limit)
            .and_then(NonZeroU32::new)
            .ok_or(LlmError::PromptTooLong {
                tokens: prompt_tokens.len(),
                limit,
            })?;
        let mut params = LlamaContextParams::default()
            .with_n_ctx(Some(n_ctx))
            .with_n_batch(n_ctx.get());
        if let Some(threads) = options.threads {
            let threads = i32::try_from(threads).map_err(runtime_error)?;
            params = params.with_n_threads(threads).with_n_threads_batch(threads);
        }
        let mut ctx = {
            let _guard = self
                .new_context
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            self.inner
                .new_context(backend, params)
                .map_err(runtime_error)?
        };

        let mut batch = LlamaBatch::new(n_ctx.get() as usize, 1);
        let last = prompt_tokens.len().saturating_sub(1);
        for (i, &token) in prompt_tokens.iter().enumerate() {
            batch
                .add(token, pos(i)?, &[0], i == last)
                .map_err(runtime_error)?;
        }
        ctx.decode(&mut batch).map_err(runtime_error)?;

        let mut grammar =
            LlamaSampler::grammar(&self.inner, grammar, "root").map_err(runtime_error)?;
        let mut rng = match options.sampling {
            Sampling::Greedy => None,
            Sampling::Random { seed, temperature } => Some((SplitMix64(seed), temperature)),
        };
        let mut bytes = Vec::new();
        // Bytes of a character not yet complete at the end of `bytes`.
        let mut pending: &[u8] = &[];
        let mut probability = 1.0_f64;
        let mut node = TokenTrie::ROOT;

        // `position` is where the token generated in this step goes.
        for (generated, position) in (1..=options.max_tokens).zip(prompt_tokens.len()..) {
            let mut candidates = ctx.token_data_array_ith(batch.n_tokens() - 1);
            grammar.apply(&mut candidates);
            let allowed: Vec<(LlamaToken, f32)> = candidates
                .data
                .iter()
                .filter(|d| d.logit().is_finite())
                // llama.cpp's grammar accepts overlong UTF-8 (e.g. F0 84 B8 AD
                // for 中), so the bytes are checked here as well.
                .filter(|d| continues_utf8(pending, self.piece(d.id())))
                .filter(|d| match &stop {
                    Stop::Trie(trie) => trie.child(node, d.id()).is_some(),
                    Stop::When(_) => true,
                })
                .map(|d| (d.id(), d.logit()))
                .collect();
            let (token, p) = match &mut rng {
                None => pick_greedy(&allowed),
                Some((rng, temperature)) => pick_random(&allowed, *temperature, rng),
            }
            .ok_or(LlmError::NoAllowedToken)?;
            if vocab.is_eog(token) {
                // The grammar only allows end-of-generation once it is
                // complete, so the stop condition disagrees with the grammar.
                return Err(LlmError::NoAllowedToken);
            }
            grammar.accept(token);
            probability *= p;
            bytes.extend_from_slice(self.piece(token));
            pending = incomplete_tail(&bytes);

            let done = match &stop {
                Stop::Trie(trie) => {
                    node = trie.child(node, token).ok_or(LlmError::NoAllowedToken)?;
                    trie.is_end(node)
                }
                Stop::When(is_complete) => std::str::from_utf8(&bytes).is_ok_and(is_complete),
            };
            if done {
                let text = String::from_utf8(bytes).map_err(runtime_error)?;
                return Ok(Generation {
                    text,
                    confidence_bp: to_basis_points(probability),
                    tokens: generated,
                });
            }

            batch.clear();
            batch
                .add(token, pos(position)?, &[0], true)
                .map_err(runtime_error)?;
            ctx.decode(&mut batch).map_err(runtime_error)?;
        }
        Err(LlmError::TooManyTokens {
            max_tokens: options.max_tokens,
        })
    }
}

/// When generation is complete.
enum Stop<'a> {
    /// When the text so far satisfies the caller.
    When(&'a dyn Fn(&str) -> bool),
    /// When the tokens so far are one of the trie's sequences; only tokens
    /// that continue one are allowed.
    Trie(&'a TokenTrie),
}

/// Token sequences sharing prefixes. Node 0 is the root.
#[derive(Debug)]
struct TokenTrie {
    nodes: Vec<TrieNode>,
}

#[derive(Debug, Default)]
struct TrieNode {
    children: Vec<(LlamaToken, usize)>,
    end: bool,
}

impl Default for TokenTrie {
    fn default() -> Self {
        Self {
            nodes: vec![TrieNode::default()],
        }
    }
}

impl TokenTrie {
    const ROOT: usize = 0;

    fn insert(&mut self, tokens: &[LlamaToken]) {
        let mut node = Self::ROOT;
        for &token in tokens {
            node = match self.child(node, token) {
                Some(next) => next,
                None => {
                    self.nodes.push(TrieNode::default());
                    let next = self.nodes.len() - 1;
                    self.nodes[node].children.push((token, next));
                    next
                }
            };
        }
        self.nodes[node].end = true;
    }

    fn child(&self, node: usize, token: LlamaToken) -> Option<usize> {
        self.nodes[node]
            .children
            .iter()
            .find(|&&(t, _)| t == token)
            .map(|&(_, next)| next)
    }

    fn is_end(&self, node: usize) -> bool {
        self.nodes[node].end
    }
}

/// Whether `piece` can follow `pending` (the incomplete character at the end
/// of the text so far) and still be valid UTF-8, now or once completed.
fn continues_utf8(pending: &[u8], piece: &[u8]) -> bool {
    let mut joined = Vec::with_capacity(pending.len() + piece.len());
    joined.extend_from_slice(pending);
    joined.extend_from_slice(piece);
    match std::str::from_utf8(&joined) {
        Ok(_) => true,
        // `None`: the input ended inside a character that is valid so far.
        Err(e) => e.error_len().is_none(),
    }
}

/// The bytes after the last complete character of `bytes`, which must be a
/// valid UTF-8 prefix.
fn incomplete_tail(bytes: &[u8]) -> &[u8] {
    match std::str::from_utf8(bytes) {
        Ok(_) => &[],
        Err(e) => &bytes[e.valid_up_to()..],
    }
}

fn pos(i: usize) -> Result<i32, LlmError> {
    i32::try_from(i).map_err(runtime_error)
}

/// Probabilities over the allowed tokens: a softmax of their logits, so the
/// mass the grammar removed is spread over what is left.
fn softmax(allowed: &[(LlamaToken, f32)], temperature: f64) -> Vec<f64> {
    let max = allowed
        .iter()
        .map(|&(_, logit)| f64::from(logit))
        .fold(f64::NEG_INFINITY, f64::max);
    let weights: Vec<f64> = allowed
        .iter()
        .map(|&(_, logit)| ((f64::from(logit) - max) / temperature).exp())
        .collect();
    let sum: f64 = weights.iter().sum();
    weights.into_iter().map(|w| w / sum).collect()
}

/// The most likely allowed token and its probability. Ties go to the lowest
/// token id, so the pick never depends on candidate order.
fn pick_greedy(allowed: &[(LlamaToken, f32)]) -> Option<(LlamaToken, f64)> {
    let probabilities = softmax(allowed, 1.0);
    allowed
        .iter()
        .zip(probabilities)
        .max_by(|((a_id, a_logit), _), ((b_id, b_logit), _)| {
            a_logit.total_cmp(b_logit).then(b_id.0.cmp(&a_id.0))
        })
        .map(|(&(token, _), p)| (token, p))
}

/// A random allowed token drawn at `temperature`, with its probability at
/// temperature 1 (the model's own distribution, which confidence is about).
fn pick_random(
    allowed: &[(LlamaToken, f32)],
    temperature: f32,
    rng: &mut SplitMix64,
) -> Option<(LlamaToken, f64)> {
    if allowed.is_empty() {
        return None;
    }
    let draw_from = softmax(allowed, f64::from(temperature.max(1e-3)));
    let model = softmax(allowed, 1.0);
    let mut target = rng.next_f64();
    for (i, p) in draw_from.iter().enumerate() {
        target -= p;
        if target < 0.0 {
            return Some((allowed[i].0, model[i]));
        }
    }
    let i = allowed.len() - 1;
    Some((allowed[i].0, model[i]))
}

fn to_basis_points(probability: f64) -> u16 {
    let bp = (probability * f64::from(MAX_BASIS_POINTS)).round();
    // Clamped to 0..=10_000 first, so the cast is exact.
    bp.clamp(0.0, f64::from(MAX_BASIS_POINTS)) as u16
}

/// Small seeded generator (SplitMix64) so random sampling is reproducible
/// without another dependency.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(logits: &[f32]) -> Vec<(LlamaToken, f32)> {
        logits
            .iter()
            .enumerate()
            .map(|(i, &l)| (LlamaToken(i as i32), l))
            .collect()
    }

    #[test]
    fn softmax_renormalises_over_allowed_tokens() {
        let p = softmax(&tokens(&[0.0, 0.0]), 1.0);
        assert_eq!(p, [0.5, 0.5]);
        let p = softmax(&tokens(&[2.0_f32.ln(), 0.0]), 1.0);
        assert!((p[0] - 2.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn greedy_picks_the_most_likely_and_breaks_ties_by_id() {
        let (token, p) = pick_greedy(&tokens(&[1.0, 3.0, 2.0])).unwrap();
        assert_eq!(token, LlamaToken(1));
        assert!(p > 0.5 && p < 1.0);
        let reversed = vec![(LlamaToken(7), 1.0), (LlamaToken(3), 1.0)];
        assert_eq!(pick_greedy(&reversed).unwrap().0, LlamaToken(3));
        assert_eq!(pick_greedy(&[]), None);
        assert_eq!(pick_greedy(&tokens(&[5.0])).unwrap().1, 1.0);
    }

    #[test]
    fn random_picks_are_reproducible_and_report_the_model_probability() {
        let allowed = tokens(&[0.0, 0.0, 0.0, 0.0]);
        let draw = |seed| {
            let mut rng = SplitMix64(seed);
            (0..20)
                .map(|_| pick_random(&allowed, 5.0, &mut rng).unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(draw(7), draw(7));
        assert!(draw(7).iter().all(|&(_, p)| p == 0.25));
        let picked: std::collections::HashSet<_> = draw(7).iter().map(|(t, _)| t.0).collect();
        assert!(picked.len() > 1);
    }

    #[test]
    fn trie_follows_shared_prefixes_to_their_ends() {
        let t = |ids: &[i32]| ids.iter().map(|&i| LlamaToken(i)).collect::<Vec<_>>();
        let mut trie = TokenTrie::default();
        trie.insert(&t(&[1, 2, 3]));
        trie.insert(&t(&[1, 2, 4]));
        trie.insert(&t(&[5]));
        let one = trie.child(TokenTrie::ROOT, LlamaToken(1)).unwrap();
        assert!(!trie.is_end(one));
        assert_eq!(trie.child(TokenTrie::ROOT, LlamaToken(2)), None);
        let two = trie.child(one, LlamaToken(2)).unwrap();
        assert!(trie.is_end(trie.child(two, LlamaToken(3)).unwrap()));
        assert!(trie.is_end(trie.child(two, LlamaToken(4)).unwrap()));
        assert!(trie.is_end(trie.child(TokenTrie::ROOT, LlamaToken(5)).unwrap()));
        assert_eq!(trie.nodes.len(), 6);
    }

    #[test]
    fn utf8_continuations_must_stay_valid() {
        let zhong = "中".as_bytes(); // E4 B8 AD
        assert!(continues_utf8(&[], b"abc"));
        assert!(continues_utf8(&[], &zhong[..1]));
        assert!(continues_utf8(&zhong[..1], &zhong[1..]));
        assert!(continues_utf8(&zhong[..2], &zhong[2..]));
        // Overlong 4-byte encoding of 中, which llama.cpp's grammar lets through.
        assert!(!continues_utf8(&[], &[0xF0, 0x84]));
        assert!(!continues_utf8(&[0xF0], &[0x84]));
        // Stray continuation byte, surrogate, byte that never appears.
        assert!(!continues_utf8(&[], &[0x80]));
        assert!(!continues_utf8(&[], &[0xED, 0xA0, 0x80]));
        assert!(!continues_utf8(&[], &[0xFF]));
        // A complete character can't be followed by a continuation byte.
        assert!(!continues_utf8(&[], &[b'a', 0xB8]));

        assert_eq!(incomplete_tail(b"ab"), b"");
        assert_eq!(incomplete_tail(&[b'a', 0xE4, 0xB8]), &[0xE4, 0xB8]);
    }

    #[test]
    fn basis_points_round_and_clamp() {
        assert_eq!(to_basis_points(1.0), 10_000);
        assert_eq!(to_basis_points(0.87654), 8765);
        assert_eq!(to_basis_points(0.0), 0);
        assert_eq!(to_basis_points(1.0 + 1e-12), 10_000);
    }

    #[test]
    fn rng_is_uniform_enough() {
        let mut rng = SplitMix64(1);
        let mean = (0..10_000).map(|_| rng.next_f64()).sum::<f64>() / 10_000.0;
        assert!((mean - 0.5).abs() < 0.02, "{mean}");
    }
}
