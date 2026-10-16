use anyhow::Result;

use super::{
    Agent,
    context::{active_start, context_tokens},
    usage::{Quota, Stats, Usage, fetch_quota, tokens_per_second, total_usage},
};
use crate::models;

impl Agent {
    pub async fn quota_request(
        &mut self,
    ) -> Result<impl Future<Output = Result<Quota>> + Send + 'static> {
        let token = self.credentials.access_token(&self.http).await?;
        Ok(fetch_quota(
            self.http.clone(),
            format!("{}/api/oauth/usage", self.api_base),
            token,
        ))
    }

    pub fn merge_quota(&mut self, quota: Quota) {
        self.quota = Quota {
            five_hour_remaining: self.quota.five_hour_remaining.or(quota.five_hour_remaining),
            seven_day_remaining: self.quota.seven_day_remaining.or(quota.seven_day_remaining),
            five_hour_reset: self.quota.five_hour_reset.or(quota.five_hour_reset),
            seven_day_reset: self.quota.seven_day_reset.or(quota.seven_day_reset),
        };
    }

    pub fn stats(&self) -> Stats {
        let mut usage = total_usage(&self.messages);
        usage.add(self.running_subagent_usage());
        Stats {
            usage,
            tokens_per_second: tokens_per_second(&self.messages),
            context_tokens: context_tokens(&self.messages[active_start(&self.messages)..]),
            context_window: models::context_window(&self.model),
            quota: self.quota,
        }
    }

    pub(super) fn running_subagent_usage(&self) -> Usage {
        let mut total = Usage::default();
        if let Ok(slots) = self.subagent_usage.lock() {
            slots.iter().for_each(|usage| total.add(*usage));
        }
        total
    }
}
