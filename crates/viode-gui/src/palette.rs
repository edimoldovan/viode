//! The command palette: the 100%-coverage discoverability surface.
//! Pure state + filtering here (unit-tested); ui.rs draws it.

use crate::actions::Action;

#[derive(Default)]
pub struct Palette {
    pub open: bool,
    pub query: String,
    pub selected: usize,
}

impl Palette {
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    /// The entries for the current query, best first.
    pub fn results(&self) -> Vec<Action> {
        filter(&self.query)
    }

    pub fn move_selection(&mut self, delta: i64, len: usize) {
        if len == 0 {
            self.selected = 0;
            return;
        }
        let cur = self.selected.min(len - 1) as i64;
        self.selected = (cur + delta).rem_euclid(len as i64) as usize;
    }

    /// The action Enter would run right now.
    pub fn chosen(&self) -> Option<Action> {
        self.results().get(self.selected.min(self.results().len().saturating_sub(1))).copied()
    }
}

/// Case-insensitive match over label + keywords. Every query word must
/// appear somewhere; matches that start the label rank first, label
/// matches beat keyword-only matches, registry order breaks ties.
pub fn filter(query: &str) -> Vec<Action> {
    let words: Vec<String> = query
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .collect();
    if words.is_empty() {
        return Action::ALL.to_vec();
    }
    let mut scored: Vec<(i64, usize, Action)> = Vec::new();
    for (i, a) in Action::ALL.iter().enumerate() {
        let label = a.label().to_lowercase();
        let keywords = a.keywords().to_lowercase();
        if !words
            .iter()
            .all(|w| label.contains(w.as_str()) || keywords.contains(w.as_str()))
        {
            continue;
        }
        let mut score = 0;
        if label.starts_with(words[0].as_str()) {
            score -= 2;
        }
        if words.iter().all(|w| label.contains(w.as_str())) {
            score -= 1;
        }
        scored.push((score, i, *a));
    }
    scored.sort_by_key(|(score, i, _)| (*score, *i));
    scored.into_iter().map(|(_, _, a)| a).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_lists_the_whole_registry_in_order() {
        assert_eq!(filter(""), Action::ALL.to_vec());
    }

    #[test]
    fn label_prefix_wins_over_keyword_matches() {
        let r = filter("sp");
        assert_eq!(r[0], Action::Split, "got {:?}", &r[..r.len().min(3)]);
    }

    #[test]
    fn premiere_vocabulary_finds_our_verbs() {
        assert_eq!(filter("razor")[0], Action::Split);
        assert!(filter("export").contains(&Action::RenderDialog));
        assert!(filter("doctor").contains(&Action::EngineCheckup));
        assert!(filter("lower third").contains(&Action::AddTitle));
    }

    #[test]
    fn every_word_must_match_somewhere() {
        assert!(filter("split banana").is_empty());
        assert!(!filter("trim in").is_empty());
    }

    #[test]
    fn selection_wraps_and_survives_shrinking_results() {
        let mut p = Palette::default();
        p.open();
        p.move_selection(-1, 5);
        assert_eq!(p.selected, 4);
        p.move_selection(1, 5);
        assert_eq!(p.selected, 0);
        // A narrower query than the selection index still yields a choice.
        p.selected = 30;
        p.query = "razor".into();
        assert_eq!(p.chosen(), Some(Action::Split));
    }
}
