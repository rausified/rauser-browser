//! Names-only evidence of lossy URL normalization. Missing evidence is legacy
//! unknown, never proof that a URL was stored without removing parameters.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct StoredUrl {
    pub url: String,
    pub stripped_params: Option<Vec<String>>,
}

/// Positional evidence for the main URL, activity modes and optional trail
/// sources. No original URL or removed value is retained.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UrlProvenance {
    pub url: Vec<String>,
    pub modes: Vec<Vec<String>>,
    pub trails: Vec<Option<Vec<String>>>,
}

pub(crate) fn valid_names(names: &[String]) -> bool {
    names.len() <= 4096
        && names.iter().all(|name| !name.is_empty())
        && names.iter().map(String::len).sum::<usize>() <= 8192
        && names.windows(2).all(|pair| pair[0] < pair[1])
}

impl UrlProvenance {
    pub fn page(url: &str, strip: &[String]) -> anyhow::Result<Self> {
        Ok(Self {
            url: crate::capture::canonical_url_with_provenance(url, strip)?
                .stripped_params
                .unwrap(),
            ..Self::default()
        })
    }

    pub fn activity(
        event: &brauser_protocol::ActivityEvent,
        strip: &[String],
    ) -> anyhow::Result<Self> {
        let names = |url: &str| -> anyhow::Result<Vec<String>> {
            Ok(crate::capture::canonical_url_with_provenance(url, strip)?
                .stripped_params
                .unwrap())
        };
        Ok(Self {
            url: names(&event.url)?,
            modes: event
                .modes
                .iter()
                .map(|mode| names(&mode.url))
                .collect::<anyhow::Result<_>>()?,
            trails: event
                .trails
                .iter()
                .map(|trail| trail.from_url.as_deref().map(names).transpose())
                .collect::<anyhow::Result<_>>()?,
        })
    }

    pub fn valid(&self, modes: usize, trails: &[bool]) -> bool {
        valid_names(&self.url)
            && self.modes.len() == modes
            && self.modes.iter().all(|names| valid_names(names))
            && self.trails.len() == trails.len()
            && self.trails.iter().zip(trails).all(|(names, present)| {
                names.is_some() == *present && names.as_ref().is_none_or(|names| valid_names(names))
            })
    }

    pub fn marker(&self) -> String {
        format!(
            " stripped={}",
            hex::encode(serde_json::to_vec(self).expect("names-only provenance"))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalization_records_exact_decoded_distinct_names_and_no_values() {
        let strip = ["utm_*".into(), "fbclid".into()];
        let stored = crate::capture::canonical_url_with_provenance(
            "https://example.com/p?keep=a%20b&%75tm_source=private&UtM_Source=other&utm_source=again&fbclid=x#fragment", &strip,
        ).unwrap();
        assert_eq!(stored.url, "https://example.com/p?keep=a%20b");
        assert_eq!(
            stored.stripped_params,
            Some(vec![
                "UtM_Source".into(),
                "fbclid".into(),
                "utm_source".into()
            ])
        );
        assert_eq!(
            crate::capture::canonical_url_with_provenance(
                "https://example.com/p?keep=1#fragment",
                &strip
            )
            .unwrap()
            .stripped_params,
            Some(Vec::new())
        );
    }

    #[test]
    fn malformed_or_unbounded_evidence_is_never_accepted() {
        assert!(!valid_names(&[String::new()]));
        assert!(!valid_names(&["a".into(), "a".into()]));
        assert!(!valid_names(&["b".into(), "a".into()]));
        assert!(!valid_names(&["x".repeat(8193)]));
        assert!(!UrlProvenance::default().valid(1, &[]));
        assert!(
            !UrlProvenance {
                trails: vec![None],
                ..UrlProvenance::default()
            }
            .valid(0, &[true])
        );
    }
}
