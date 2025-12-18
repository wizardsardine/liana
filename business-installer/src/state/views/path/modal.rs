/// Timelock unit for display/input
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimelockUnit {
    Blocks,
    #[default]
    Hours,
    Days,
    Months,
}

impl TimelockUnit {
    /// Blocks per unit (1 block ≈ 10 minutes)
    pub fn blocks_per_unit(self) -> u64 {
        match self {
            TimelockUnit::Blocks => 1,
            TimelockUnit::Hours => 6,
            TimelockUnit::Days => 144,
            TimelockUnit::Months => 4380,
        }
    }

    /// Convert blocks to this unit (returns the value)
    pub fn from_blocks(self, blocks: u64) -> u64 {
        blocks / self.blocks_per_unit()
    }

    /// Convert a value in this unit to blocks
    pub fn to_blocks(self, value: u64) -> u64 {
        value * self.blocks_per_unit()
    }

    /// All available units
    pub const ALL: [TimelockUnit; 4] = [
        TimelockUnit::Blocks,
        TimelockUnit::Hours,
        TimelockUnit::Days,
        TimelockUnit::Months,
    ];
}

impl std::fmt::Display for TimelockUnit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TimelockUnit::Blocks => write!(f, "blocks"),
            TimelockUnit::Hours => write!(f, "hours"),
            TimelockUnit::Days => write!(f, "days"),
            TimelockUnit::Months => write!(f, "months"),
        }
    }
}

/// State for Edit Path modal (handles key selection, threshold, and timelock)
#[derive(Debug, Clone)]
pub struct EditPathModalState {
    pub is_primary: bool,
    pub path_index: Option<usize>, // None for primary, Some(index) for secondary
    pub selected_key_ids: Vec<u8>, // Keys currently selected for this path
    pub threshold: String,
    pub timelock_value: Option<String>, // None for primary paths, Some for secondary
    pub timelock_unit: TimelockUnit,    // Unit for timelock display
}

