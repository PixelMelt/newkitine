mod template;

pub use template::DescriptionTemplate;
#[cfg(test)]
pub use template::description_variables;

use std::sync::{Arc, RwLock};

use soulseek::client::{ProfileRequest, ProfileSource};

use crate::transfers::TransferTotals;

pub struct Profile {
    template: RwLock<DescriptionTemplate>,
    totals: Arc<TransferTotals>,
}

impl Profile {
    pub fn new(template: DescriptionTemplate, totals: Arc<TransferTotals>) -> Self {
        Self {
            template: RwLock::new(template),
            totals,
        }
    }

    pub fn set_template(&self, template: DescriptionTemplate) {
        *self.template.write().unwrap() = template;
    }
}

impl ProfileSource for Profile {
    fn description(&self, request: &ProfileRequest<'_>) -> String {
        if let Some(reason) = request.user_ban_reason {
            return format!("You are not allowed to download my shared files.\nReason: {reason}");
        }
        self.template
            .read()
            .unwrap()
            .render(request, &self.totals.snapshot())
    }
}
