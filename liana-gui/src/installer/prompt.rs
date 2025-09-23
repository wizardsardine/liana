pub const BACKUP_DESCRIPTOR_MESSAGE: &str = "This backup is required to recover your funds.
Click “Back Up Descriptor” to download an encrypted file of your wallet configuration and store it in safe, accessible places.
You can also copy the plain-text descriptor string, but it’s less private.
⚠️ This file does not include your seed phrase(s). Back those up separately.";
pub const BACKUP_DESCRIPTOR_HELP: &str = "In Bitcoin, to spend from a wallet that isn't a standard single-key setup, you need both your private keys (usually stored as seed words) to sign transactions, and your wallet descriptor to locate your coins — like a map of your addresses.
Without the descriptor, your wallet may not find your coins — even if you still have the keys. 
When you click “Back Up Descriptor”, Liana creates an encrypted file that can only be decrypted using one of your wallet’s public keys. 
Liana handles this automatically during the restore of a wallet process by asking you to connect a device or enter a key.
This file is safer and more private than copying the descriptor manually.";
pub const REGISTER_DESCRIPTOR_HELP: &str = "To be used with the wallet, a signing device needs the descriptor. If the descriptor contains one or more keys imported from an external signing device, the descriptor must be registered on it. Registration confirms that the device is able to handle the policy. Registration on a device is not a substitute for backing up the descriptor.";
pub const MNEMONIC_HELP: &str = "A hot key generated on this computer was used for creating this wallet. It needs to be backed up. \n Keep it in a safe place. Never share it with anyone.";
pub const RECOVER_MNEMONIC_HELP: &str = "If you were using a hot key (a key stored on the computer) in your wallet, you will need to recover it from mnemonics to be able to sign transactions again. Otherwise you can directly go the next step.";
