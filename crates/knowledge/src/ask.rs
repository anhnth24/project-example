#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerMode {
    OfflineExtractive,
    FallbackExtractive,
    LocalLlm,
    CloudLlm,
    SubscriptionCli,
}

impl AnswerMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OfflineExtractive => "offline_extractive",
            Self::FallbackExtractive => "fallback_extractive",
            Self::LocalLlm => "local_llm",
            Self::CloudLlm => "cloud_llm",
            Self::SubscriptionCli => "subscription_cli",
        }
    }
}
