use std::collections::HashMap;

pub struct Searches {
    active: HashMap<u32, String>,
}

impl Searches {
    pub fn new() -> Self {
        Self {
            active: HashMap::new(),
        }
    }

    pub fn add(&mut self, token: u32, query: String) {
        self.active.insert(token, query);
    }

    pub fn remove(&mut self, token: u32) -> Option<String> {
        self.active.remove(&token)
    }

    pub fn query(&self, token: u32) -> Option<&String> {
        self.active.get(&token)
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
