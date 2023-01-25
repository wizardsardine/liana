pub const BACKUP_DESCRIPTOR_MESSAGE: &str = "The descriptor is necessary to recover your funds. The backup of your key (via mnemonics, sometimes called 'seed words') is not enough. Please make sure you have backed up both your private key and your descriptor.";
pub const BACKUP_DESCRIPTOR_HELP: &str = "In Bitcoin, the coins are locked using a Script (related to the 'address'). In order to recover your funds you need both to know the Scripts you have participated in (your 'addresses'), and be able to sign a transaction that spends from those. For the ability to sign you backup your private key, this is your mnemonics ('seed words'). For finding the coins that belongs to you you backup a template of your Script ( / 'addresses'), this is your descriptor. Note however the descriptor needs not be as securely stored as the private key. A thief that steals your descriptor but not your private key will not be able to steal your funds.";
pub const DEFINE_DESCRIPTOR_PRIMATRY_PATH_TOOLTIP: &str =
    "This is the keys that can spend received coins immediately,\n with no time restriction.";
pub const DEFINE_DESCRIPTOR_SEQUENCE_TOOLTIP: &str =
    "Number of blocks after a coin is received \nfor which the recovery path is not available";
pub const DEFINE_DESCRIPTOR_FINGERPRINT_TOOLTIP: &str =
    "The alias is applied on all the keys derived from the same seed";
