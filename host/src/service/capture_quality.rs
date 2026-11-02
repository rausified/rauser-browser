//! Flags page captures that look broken, for log review (DESIGN.md §15).
//!
//! The service checks the text the extension read before a Summarize & log
//! or task run is queued. A capture that looks wrong is logged as one
//! `page capture looks suspect` warning; the run itself is unchanged. The
//! 2026-11-01 case: every Word for the web capture had read 275 characters
//! of ribbon keytips ("Control Period, fzczazmzs02h, zzh, ch, fph, …") for
//! days before anyone noticed. The checks are heuristics tuned so that text
//! trips them and ordinary prose, code and acronym-heavy notes do not.

use super::office::OfficeKind;

/// An Office capture shorter than this many characters is thin.
pub const THIN_OFFICE_CHARS: usize = 500;
/// The token-like and comma-list checks need at least this many tokens.
pub const MIN_TOKENS: usize = 20;
/// Share of lowercase or mixed-case ASCII tokens with no vowel at which the
/// text is token-like. English prose sits well under a tenth.
pub const VOWELLESS_RATIO: f64 = 0.4;
/// Commas per token at which the text is one comma-separated list.
pub const COMMAS_PER_TOKEN: f64 = 0.6;
/// Characters of the capture kept in the log line.
pub const EXCERPT_CHARS: usize = 300;

/// Why a capture looks suspect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// No text at all.
    Empty,
    /// Mostly short vowel-less tokens (`fph, zzh, 1h`): UI ids, not words.
    TokenLike,
    /// About one comma per token.
    CommaList,
    /// An Office document page that yielded under [`THIN_OFFICE_CHARS`].
    ThinOffice,
}

impl Reason {
    pub fn name(self) -> &'static str {
        match self {
            Reason::Empty => "empty",
            Reason::TokenLike => "token_like",
            Reason::CommaList => "comma_list",
            Reason::ThinOffice => "thin_office",
        }
    }
}

/// The measures behind a verdict, all logged.
#[derive(Clone, Debug, PartialEq)]
pub struct Assessment {
    pub reasons: Vec<Reason>,
    pub chars: usize,
    pub tokens: usize,
    pub vowelless_ratio: f64,
    pub commas_per_token: f64,
}

impl Assessment {
    /// The reasons as one comma-separated field value.
    pub fn reason_names(&self) -> String {
        self.reasons
            .iter()
            .map(|reason| reason.name())
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// Measures `content`, the text a page run captured; `office` is the Office
/// document kind the host recognised from the page address, if any. Returns
/// `None` when nothing looks wrong.
pub fn assess(content: &str, office: Option<OfficeKind>) -> Option<Assessment> {
    let text = content.trim();
    let chars = text.chars().count();
    let tokens: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c == ',')
        .map(|token| token.trim_matches(|c: char| c.is_ascii_punctuation()))
        .filter(|token| !token.is_empty())
        .collect();
    // Acronyms (all capitals: DNS, HTTP, ZTA) and pure numbers are left out,
    // since notes are full of them; keytip ids are lowercase.
    let wordish: Vec<&str> = tokens
        .iter()
        .copied()
        .filter(|token| token.chars().all(|c| c.is_ascii_alphanumeric()))
        .filter(|token| token.chars().any(|c| c.is_ascii_lowercase()))
        .collect();
    let vowelless = wordish
        .iter()
        .filter(|token| !token.chars().any(|c| "aeiouyAEIOUY".contains(c)))
        .count();
    let ratio = |part: usize, whole: usize| {
        if whole == 0 {
            0.0
        } else {
            part as f64 / whole as f64
        }
    };
    let vowelless_ratio = ratio(vowelless, wordish.len());
    let commas_per_token = ratio(text.matches(',').count(), tokens.len());

    let mut reasons = Vec::new();
    if chars == 0 {
        reasons.push(Reason::Empty);
    }
    if wordish.len() >= MIN_TOKENS && vowelless_ratio >= VOWELLESS_RATIO {
        reasons.push(Reason::TokenLike);
    }
    if tokens.len() >= MIN_TOKENS && commas_per_token >= COMMAS_PER_TOKEN {
        reasons.push(Reason::CommaList);
    }
    if office.is_some() && chars > 0 && chars < THIN_OFFICE_CHARS {
        reasons.push(Reason::ThinOffice);
    }
    (!reasons.is_empty()).then_some(Assessment {
        reasons,
        chars,
        tokens: tokens.len(),
        vowelless_ratio: (vowelless_ratio * 100.0).round() / 100.0,
        commas_per_token: (commas_per_token * 100.0).round() / 100.0,
    })
}

/// The first [`EXCERPT_CHARS`] characters of `content` on one line.
pub fn excerpt(content: &str) -> String {
    let line = content.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(EXCERPT_CHARS) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The capture every Word for the web run logged before 2026-11-01.
    const KEYTIPS: &str = "Control Period, fzczazmzs02h, zzh, ch, fph, fsh, fgh, fkh, 1h, 2h, 3h, ih, \
        fch, eh, uh, nh, hlh, aoh, aih, ath, bh, psh, kh, pgh, lh, lh, znh, dh, syh, suh, aph, \
        00zrhnpsrwe00h, ffh, 4h, 5h, 6h, 7h, eh, hlh, kh, sih, tlh, trh, fmh, pgh, apzr, czr, szr, \
        ozr, azr, gqh, y3h, y4";

    const PROSE: &str = "I read the underlying Word document through SharePoint. It maps Cisco \
        Secure Connect ZTA requirements across onboarding, client enrollment, private resources, \
        traffic steering, policy, posture, and reporting. Specific requirements include Umbrella \
        and Meraki organization linking, API integration, HTTP/2 and HTTP/3 SNI proxy integration, \
        and distinct browser-based and client-based access constraints.";

    const CODE: &str = "fn main() {\n    let mut cfg = Config::default();\n    cfg.set_dir(\"src\");\n    \
        std::fs::write(\"out.txt\", b\"ok\").unwrap();\n    println!(\"{}\", cfg.len());\n}\n\
        $ cd src && npm run build && ls -la dist && git status --short";

    #[test]
    fn the_word_keytip_capture_is_token_like_and_a_comma_list() {
        let found = assess(KEYTIPS, Some(OfficeKind::Word)).unwrap();
        assert_eq!(
            found.reasons,
            [Reason::TokenLike, Reason::CommaList, Reason::ThinOffice]
        );
        assert_eq!(found.reason_names(), "token_like,comma_list,thin_office");
        assert_eq!(found.chars, KEYTIPS.chars().count());
        assert!(found.vowelless_ratio >= VOWELLESS_RATIO, "{found:?}");
        assert!(found.commas_per_token >= COMMAS_PER_TOKEN, "{found:?}");
        // The same text off an Office page is still suspect.
        assert_eq!(
            assess(KEYTIPS, None).unwrap().reasons,
            [Reason::TokenLike, Reason::CommaList]
        );
    }

    #[test]
    fn prose_code_and_acronyms_are_not_suspect() {
        assert_eq!(assess(PROSE, None), None);
        assert_eq!(assess(CODE, None), None);
        let acronyms = "ZTA CLAP BAP SNI DNS HTTP TCP UDP TLS VPN SASE SWG CASB DLP ".repeat(3);
        assert_eq!(assess(&acronyms, None), None);
        let long = PROSE.repeat(3);
        assert_eq!(assess(&long, Some(OfficeKind::Word)), None);
    }

    #[test]
    fn empty_and_thin_office_captures_are_suspect() {
        assert_eq!(assess("  \n ", None).unwrap().reasons, [Reason::Empty]);
        assert_eq!(
            assess("", Some(OfficeKind::PowerPoint)).unwrap().reasons,
            [Reason::Empty]
        );
        assert_eq!(
            assess("Suite bar", Some(OfficeKind::Excel))
                .unwrap()
                .reasons,
            [Reason::ThinOffice]
        );
        // A short non-Office page is fine.
        assert_eq!(assess("Suite bar", None), None);
    }

    #[test]
    fn excerpt_is_one_bounded_line() {
        assert_eq!(excerpt("  a\n\n b\tc "), "a b c");
        let long = "é".repeat(EXCERPT_CHARS + 10);
        let cut = excerpt(&long);
        assert_eq!(cut.chars().count(), EXCERPT_CHARS + 1);
        assert!(cut.ends_with('…'));
        assert_eq!(
            excerpt(&"x".repeat(EXCERPT_CHARS)),
            "x".repeat(EXCERPT_CHARS)
        );
    }
}
