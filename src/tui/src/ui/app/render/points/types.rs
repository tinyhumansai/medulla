//! Dummy data records used by the TokenMaxxxing design.

/// One deterministic daily challenge rendered on the bounty page.
pub(super) struct DailyBounty {
    /// Short challenge name.
    pub(super) title: &'static str,
    /// Completion condition.
    pub(super) detail: &'static str,
    /// Point award.
    pub(super) reward: &'static str,
    /// Current progress.
    pub(super) progress: u8,
    /// Progress required to complete the bounty.
    pub(super) target: u8,
}

/// Dummy bounties used while the program rules and backend contract are shaped.
pub(super) const DAILY_BOUNTIES: [DailyBounty; 3] = [
    DailyBounty {
        title: "Ship something useful",
        detail: "Complete one agent task",
        reward: "+120 pts",
        progress: 1,
        target: 1,
    },
    DailyBounty {
        title: "Close the loop",
        detail: "Finish three assigned tasks",
        reward: "+80 pts",
        progress: 2,
        target: 3,
    },
    DailyBounty {
        title: "Explorer bonus",
        detail: "Try a new workflow",
        reward: "+200 pts",
        progress: 0,
        target: 1,
    },
];

/// One GitHub user in the TokenMaxxxing season standings.
pub(super) struct LeaderboardEntry {
    /// Current season rank.
    pub(super) rank: &'static str,
    /// Public GitHub handle.
    pub(super) github: &'static str,
    /// LLM tokens burned through Medulla this season.
    pub(super) tokens: &'static str,
    /// Distinct days active on Medulla this season.
    pub(super) days: &'static str,
    /// Current consecutive-day streak.
    pub(super) streak: &'static str,
    /// Notable live standing or projected reward.
    pub(super) status: &'static str,
    /// Whether this row represents the current user.
    pub(super) is_you: bool,
}

/// Dummy season standings combining token burn and active-day commitment.
pub(super) const LEADERBOARD: [LeaderboardEntry; 7] = [
    LeaderboardEntry {
        rank: "1",
        github: "@mira-dev",
        tokens: "48.2M",
        days: "27",
        streak: "14d",
        status: "daily lead",
        is_you: false,
    },
    LeaderboardEntry {
        rank: "2",
        github: "@byteforge",
        tokens: "44.8M",
        days: "25",
        streak: "9d",
        status: "top burner",
        is_you: false,
    },
    LeaderboardEntry {
        rank: "3",
        github: "@luna-ops",
        tokens: "42.1M",
        days: "29",
        streak: "21d",
        status: "days leader",
        is_you: false,
    },
    LeaderboardEntry {
        rank: "4",
        github: "@agent-olive",
        tokens: "36.7M",
        days: "22",
        streak: "6d",
        status: "top 5",
        is_you: false,
    },
    LeaderboardEntry {
        rank: "5",
        github: "@niko-builds",
        tokens: "31.4M",
        days: "20",
        streak: "4d",
        status: "top 5",
        is_you: false,
    },
    LeaderboardEntry {
        rank: "⋮",
        github: "",
        tokens: "",
        days: "",
        streak: "",
        status: "",
        is_you: false,
    },
    LeaderboardEntry {
        rank: "12",
        github: "@you",
        tokens: "9.8M",
        days: "12",
        streak: "7d",
        status: "↑ 3 places",
        is_you: true,
    },
];

/// One completed daily TokenMaxxxing competition.
pub(super) struct PreviousWinner {
    /// Day the leaderboard closed.
    pub(super) day: &'static str,
    /// Winning GitHub handle.
    pub(super) github: &'static str,
    /// Tokens burned during that day.
    pub(super) tokens: &'static str,
    /// Reward paid for the win.
    pub(super) reward: &'static str,
}

/// Dummy recent winners showing how daily recognition will read.
pub(super) const PREVIOUS_WINNERS: [PreviousWinner; 4] = [
    PreviousWinner {
        day: "Jul 26",
        github: "@mira-dev",
        tokens: "3.4M",
        reward: "$25 + 1,000 pts",
    },
    PreviousWinner {
        day: "Jul 25",
        github: "@byteforge",
        tokens: "3.1M",
        reward: "$25 + 1,000 pts",
    },
    PreviousWinner {
        day: "Jul 24",
        github: "@luna-ops",
        tokens: "2.9M",
        reward: "$25 + 1,000 pts",
    },
    PreviousWinner {
        day: "Jul 23",
        github: "@agent-olive",
        tokens: "2.7M",
        reward: "$25 + 1,000 pts",
    },
];
