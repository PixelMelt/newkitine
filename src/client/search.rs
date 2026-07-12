use std::collections::HashSet;

pub struct Searches {
    active: HashSet<u32>,
}

impl Searches {
    pub fn new() -> Self {
        Self {
            active: HashSet::new(),
        }
    }

    pub fn add(&mut self, token: u32) {
        self.active.insert(token);
    }

    pub fn remove(&mut self, token: u32) {
        self.active.remove(&token);
    }

    pub fn contains(&self, token: u32) -> bool {
        self.active.contains(&token)
    }

    pub fn clear(&mut self) {
        self.active.clear();
    }
}

pub fn sanitize_search_term(text: &str) -> String {
    text.split_whitespace()
        .filter(|word| *word != "-")
        .collect::<Vec<_>>()
        .join(" ")
}
