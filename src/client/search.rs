use super::punctuation::split_words;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchScope {
    Global,
    Room(String),
    Buddies,
    User(String),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct SearchQuery {
    pub(super) transmitted: String,
    included: Vec<String>,
    excluded: Vec<String>,
}

impl SearchQuery {
    pub(super) fn parse(term: &str) -> Self {
        let term = term.trim();
        let words = quoted_words(term)
            .unwrap_or_else(|| term.split_whitespace().map(str::to_owned).collect());
        let mut included = Vec::new();
        let mut excluded = Vec::new();
        let mut transmitted = Vec::new();
        for word in words {
            if let Some(partial) = word.strip_prefix('*').filter(|rest| !rest.is_empty()) {
                included.push(partial.to_lowercase());
                transmitted.push(word);
            } else if let Some(exclusion) = word.strip_prefix('-').filter(|rest| !rest.is_empty()) {
                excluded.push(exclusion.to_lowercase());
                transmitted.push(word);
            } else if let Some(phrase) = word
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
                .filter(|phrase| !phrase.is_empty())
            {
                included.push(phrase.to_lowercase());
                transmitted.extend(
                    strip_unsearchable(phrase)
                        .split_whitespace()
                        .map(str::to_owned),
                );
            } else {
                let cleaned = strip_unsearchable(&word)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                if cleaned.is_empty() {
                    continue;
                }
                included.extend(split_words(&cleaned).map(str::to_lowercase));
                transmitted.push(cleaned);
            }
        }
        let transmitted = transmitted.join(" ").trim().to_owned();
        Self {
            transmitted: if transmitted.is_empty() {
                term.to_owned()
            } else {
                transmitted
            },
            included,
            excluded,
        }
    }

    pub(super) fn matches(&self, path: &str) -> bool {
        let path = path.to_lowercase();
        !self
            .excluded
            .iter()
            .any(|word| path.contains(word.as_str()))
            && self
                .included
                .iter()
                .all(|word| path.contains(word.as_str()))
    }
}

fn quoted_words(term: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_quotes = false;
    for c in term.chars() {
        if in_quotes {
            word.push(c);
            if c == '"' {
                words.push(std::mem::take(&mut word));
                in_quotes = false;
            }
        } else if matches!(c, ' ' | '\t' | '\r' | '\n') {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            in_quotes = c == '"' && word.is_empty();
            word.push(c);
        }
    }
    if in_quotes {
        return None;
    }
    if !word.is_empty() {
        words.push(word);
    }
    Some(words)
}

fn strip_unsearchable(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_punctuation()
                || matches!(
                    c,
                    '\u{2010}'
                        | '\u{2013}'
                        | '\u{2014}'
                        | '\u{2019}'
                        | '\u{201C}'
                        | '\u{201D}'
                        | '\u{2026}'
                )
            {
                ' '
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn punctuation_in_plain_words_is_replaced_before_sending() {
        let query = SearchQuery::parse("  AC/DC  back_in-black ");
        assert_eq!(query.transmitted, "AC DC back in black");
        assert_eq!(query.included, words(&["ac", "dc", "back", "in", "black"]));
        assert!(query.excluded.is_empty());
    }

    #[test]
    fn exclusions_and_partials_are_sent_verbatim() {
        let query = SearchQuery::parse("nicotine -music *tine");
        assert_eq!(query.transmitted, "nicotine -music *tine");
        assert_eq!(query.included, words(&["nicotine", "tine"]));
        assert_eq!(query.excluded, words(&["music"]));
    }

    #[test]
    fn phrases_lose_their_quotes_but_filter_as_a_whole() {
        let query = SearchQuery::parse("\"Daft Punk\" live");
        assert_eq!(query.transmitted, "Daft Punk live");
        assert_eq!(query.included, words(&["daft punk", "live"]));
        assert!(query.matches("music\\Daft Punk - Live.mp3"));
        assert!(!query.matches("music\\Daft - Punk Live.mp3"));
    }

    #[test]
    fn quotes_only_open_a_phrase_at_the_start_of_a_word() {
        let query = SearchQuery::parse("a\"b c\"d \"x y\"z");
        assert_eq!(query.transmitted, "a b c d x y z");
        assert_eq!(query.included, words(&["a", "b", "c", "d", "x y", "z"]));
    }

    #[test]
    fn an_unclosed_quote_splits_on_whitespace() {
        let query = SearchQuery::parse("\"daft punk");
        assert_eq!(query.transmitted, "daft punk");
        assert_eq!(query.included, words(&["daft", "punk"]));
    }

    #[test]
    fn a_term_of_only_special_characters_is_sent_unchanged() {
        assert_eq!(SearchQuery::parse(" - ").transmitted, "-");
        assert_eq!(SearchQuery::parse("   ").transmitted, "");
    }

    #[test]
    fn results_need_every_included_word_and_no_excluded_word() {
        let query = SearchQuery::parse("nicotine -music");
        assert!(query.matches("Share\\Nicotine\\readme.txt"));
        assert!(!query.matches("Share\\Nicotine\\music.flac"));
        assert!(!query.matches("Share\\other\\readme.txt"));
    }
}
