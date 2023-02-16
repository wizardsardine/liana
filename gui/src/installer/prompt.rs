pub const BACKUP_DESCRIPTOR_MESSAGE: &str = "The descriptor is necessary to recover your funds. The backup of your key (via mnemonics, sometimes called 'seed words') is not enough. Please make sure you have backed up both your private key and your descriptor.";
pub const BACKUP_DESCRIPTOR_HELP: &str = "In Bitcoin, the coins are locked using a Script (related to the 'address'). In order to recover your funds you need both to know the Scripts you have participated in (your 'addresses'), and be able to sign a transaction that spends from those. For the ability to sign you backup your private key, this is your mnemonics ('seed words'). For finding the coins that belongs to you you backup a template of your Script ( / 'addresses'), this is your descriptor. Note however the descriptor needs not be as securely stored as the private key. A thief that steals your descriptor but not your private key will not be able to steal your funds.";
pub const DEFINE_DESCRIPTOR_PRIMATRY_PATH_TOOLTIP: &str =
    "This is the keys that can spend received coins immediately,\n with no time restriction.";
pub const DEFINE_DESCRIPTOR_SEQUENCE_TOOLTIP: &str =
    "Number of blocks after a coin is received \nfor which the recovery path is not available";
pub const DEFINE_DESCRIPTOR_FINGERPRINT_TOOLTIP: &str =
    "The alias is applied on all the keys derived from the same seed";
pub const REGISTER_DESCRIPTOR_HELP: &str = "To be used with the wallet, a device needs the descriptor. If the descriptor contains one or more keys imported from an external signing device, the descriptor must be registered on it. Registration on a device is not a substitute for backing up the descriptor.";
pub const MNEMONIC_HELP: &str = "A hot key generated on this computer was used for creating this wallet. It needs to be backed up. \n Keep it in a safe place. Never share it with anyone.";
pub const RECOVER_MNEMONIC_HELP: &str = "If you were using a hot key (a key stored on the computer) in your wallet, you will need to recover it from mnemonics to be able to sign transactions again. Otherwise you can directly go the next step.";
