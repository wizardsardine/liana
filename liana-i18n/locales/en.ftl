settings-language = Language
settings-language-description = Choose the language used by the application.
settings-fiat-price = Fiat price:
settings-fiat-price-tooltip = Fiat price data is provided by third-party services. Availability and accuracy are not guaranteed.
settings-exchange-rate-source = Exchange rate source:
settings-currency = Currency:
home-balance = Balance
home-payment-history = Payment History
menu-dashboard = Dashboard
menu-receive = Receive
menu-drafts-approvals = Drafts & Approvals
menu-transactions = Transactions
menu-settings = Settings
settings-section-general = General
settings-section-node = Node
settings-section-backend = Backend
settings-section-wallet = Wallet
settings-section-import-export = Import/Export
settings-section-about = About
settings-import-wallet = Import wallet
settings-import-wallet-description = Upload a backup file to update wallet info.
settings-export-wallet = Export wallet
settings-export-wallet-description = File (not encrypted) with wallet info useful to sync labels and data on other devices.
settings-export-labels = BIP 329 labels
settings-export-labels-description = BIP 329 label export, compatible with other wallets.
settings-export-transactions = Transactions table
settings-export-transactions-description = .CSV file of past transactions, for accounting purposes.
settings-export-descriptor = Descriptor only - plain-text
settings-export-descriptor-description = Plain-text (not encrypted) descriptor file only, to use with other wallets.
settings-export-encrypted-descriptor = Encrypted descriptor
settings-export-encrypted-descriptor-description = .bed file, can be decrypted with one of your signing devices or xpubs.
menu-coins-utxos = Coins/UTXOs
menu-send = Send
menu-recovery = Recovery
tab-close = Close
tab-split = Split
tab-installer = Installer
tab-loading = Loading...
tab-launcher = Launcher
tab-login = Login
common-select = Select
common-login = Login
common-token = Token
common-go-back = Go back
common-connection-failed = Connection failed
common-fetching = Fetching ...
common-see-more = See more
common-address-label = Address:
launcher-back-to-wallet-list = Back to wallet list
launcher-share-xpubs = Share Xpubs
launcher-welcome-back = Welcome back
launcher-welcome = Welcome
launcher-add-wallet = Add wallet
launcher-create-new-wallet = Create a new Liana wallet
launcher-add-existing-wallet = Add an existing Liana wallet
launcher-default-wallet-name = My Liana {$network} wallet
launcher-delete-wallet = Delete wallet
launcher-delete-local-config-question = Are you sure you want to delete the local configuration for wallet
launcher-delete-all-data-question = Are you sure you want to delete the configuration and all associated data for wallet
launcher-delete-node-not-affected-this-network = (The Liana-managed Bitcoin node for this network will not be affected by this action.)
launcher-delete-node-not-affected = (If you are using a Liana-managed Bitcoin node, it will not be affected by this action.)
launcher-delete-warning-irreversible = WARNING: This cannot be undone.
launcher-delete-title-alias = Delete configuration for {$alias} (Liana-{$checksum})
launcher-delete-title = Delete configuration for Liana-{$checksum}
launcher-delete-connect-all-members = Also permanently delete this wallet from Liana Connect (for all members).
launcher-delete-connect-disassociate = Also disassociate {$email} from this Liana Connect wallet.
launcher-wallet-deleted = Wallet successfully deleted
lianalite-token-expired = Token is expired or is invalid
lianalite-wallet-deleted = This wallet was deleted by its creator for all participants and cannot be opened. To access it again, restore it using a backup file or the wallet descriptor.
lianalite-auth-sent = An authentication was sent to your email:
lianalite-token-invalid = Token is not valid
lianalite-resend-token = Resend token
receive-verify-on-device = Verify on hardware device
receive-show-qr = Show QR Code
receive-generate-address = Generate address
receive-generate-new-address-help = Always generate a new address for each deposit.
receive-previous-addresses = Previously generated addresses still awaiting deposit
receive-derivation-index = Derivation index:
receive-select-device = Select device to verify address on:
common-import = Import
common-processing = Processing...
common-new = New
common-export = Export
common-confirm = Confirm
common-next = Next
common-self-transfer = Self-transfer
common-from = From
common-no-label = No label
common-feerate = Feerate
psbts-insert-psbt = Insert PSBT:
psbts-base64-warning = Please enter a base64 encoded PSBT
psbts-imported = PSBT is imported
coins-recovery-available = One or more recovery paths are available
coins-first-recovery-in-blocks = First recovery path will be available in {$blocks} blocks
coins-address-label = Address label:
coins-deposit-transaction-label = Deposit transaction label:
coins-outpoint = Outpoint:
coins-block-height = Block height:
coins-spend-txid = Spend txid:
coins-spend-block-height = Spend block height:
coins-not-in-block = Not in a block
coins-refresh-coin = Refresh coin
recovery-info = Recover your funds by sending them to another wallet if you have lost access to your primary spending path.
recovery-none-available = No recovery path is currently available.
recovery-paths-available = { $count ->
    [one] 1 recovery path is available:
   *[other] {$count} recovery paths are available:
}
recovery-signatures-from = { $count ->
    [one] 1 signature from
   *[other] {$count} signatures from
}
recovery-can-recover = can recover
recovery-coins-total = { $count ->
    [one] 1 coin totalling
   *[other] {$count} coins totalling
}
transactions-rbf-cancel-help = Replace the transaction with one paying a higher feerate that sends the coins back to your wallet. There is no guarantee the original transaction won't get mined first. New inputs may be used for the replacement transaction.
transactions-rbf-bump-help = Replace the transaction with one paying a higher feerate to incentivize faster confirmation. New inputs may be used for the replacement transaction.
transactions-replacement = Transaction replacement
transactions-rbf-invalidates-some = WARNING: Replacing this transaction will invalidate some later payments.
transactions-rbf-invalidates-one = WARNING: Replacing this transaction will invalidate a later payment.
transactions-rbf-descendants-some = The following transactions are spending one or more outputs from the transaction to be replaced and will be dropped when the replacement is broadcast, along with any other transactions that depend on them:
transactions-rbf-descendants-one = The following transaction is spending one or more outputs from the transaction to be replaced and will be dropped when the replacement is broadcast, along with any other transactions that depend on it:
transactions-rbf-feerate-warning = Feerate must be greater than previous value and less than or equal to 1000 sats/vbyte
transactions-rbf-created = Replacement PSBT created successfully and ready to be signed
transactions-go-to-replacement = Go to replacement
transactions-transaction = Transaction
transactions-incoming = Incoming transaction
transactions-outgoing = Outgoing transaction
transactions-miner-fee = Miner fee:
transactions-bump-fee = Bump fee
transactions-cancel = Cancel transaction
transactions-cancel-tooltip = Best effort attempt at double spending an unconfirmed outgoing transaction
transactions-date = Date:
transactions-txid = Txid:
common-delete = Delete
common-previous = Previous
common-save = Save
common-clear = Clear
common-address = Address
spend-batch-label = Batch label
spend-label-too-long = Invalid label length, cannot be greater than 100
spend-duplicate-addresses = Two payment addresses are the same
spend-add-payment = Add payment
spend-feerate-placeholder = 42 (in sats/vbyte)
spend-feerate-warning = Feerate must be an integer less than or equal to 1000 sats/vbyte
spend-fee = Fee:
spend-feerate = Feerate:
spend-selected = selected
spend-select-one-coin = Select at least one coin.
spend-check-max-recipient = Check max amount for recipient.
spend-left-to-select = left to select
spend-feerate-needed = Feerate needs to be set.
spend-add-recipient-details = Add recipient details.
spend-select-or-add-funds = Select or add more funds.
spend-coins-selection = Coins selection
spend-invalid-address = Invalid address (maybe it is for another network?)
spend-description = Description
spend-payment-label = Payment label
spend-amount-btc = Amount (BTC)
spend-btc-placeholder = 0.001 (in BTC)
spend-invalid-amount = Invalid amount. (Note amounts lower than 0.000005 BTC are invalid.)
spend-fiat-placeholder = Enter amount in {$currency}
spend-max-tooltip = Total amount remaining after paying fee and any other recipients
settings-import-export-description = A collection of the export and import functions present in Liana.
settings-other-formats = Other formats
settings-version = Version
settings-grant-wallet-access = Grant access to wallet to another user
settings-user-email = User email
settings-email-invalid = Email is invalid
settings-invitation-sent = Invitation was sent
settings-send-invitation = Send invitation
settings-connect-own-node = I want to connect to my own node
settings-network = Network:
settings-block-height = Block Height:
common-accept = Accept
common-descriptor-label = Descriptor:
common-descriptor = Descriptor
common-or = or
common-something-wrong = Something wrong happened
installer-load-previous-wallet = Load a previously used wallet
installer-no-current-wallets = You have no current wallets
installer-load-shared-wallet = Load a shared wallet
installer-shared-wallet-help = If you received an invitation to join a shared wallet
installer-invitation-token-help = Type the invitation token you received by email
installer-accept-invitation-for = Accept invitation for wallet:
installer-paste-invitation = Paste invitation:
installer-invitation = Invitation
installer-invitation-invalid = Invitation token is invalid or expired
installer-load-from-descriptor = Load a wallet from descriptor
installer-load-from-descriptor-help = Creates a new wallet from the descriptor
installer-descriptor-invalid = Either descriptor is invalid or incompatible with network
installer-import-descriptor = Import descriptor
common-cancel = Cancel
common-overwrite = Overwrite
common-ignore = Ignore
hw-descriptor-not-registered = The wallet descriptor is not registered on the device.
 You can register it in the settings.
hw-not-in-spending-path = This signing device is not part of this spending path.
hw-no-taproot-miniscript = Device firmware version does not support taproot miniscript
hw-display-address-unavailable = Liana cannot request the device to display the address.
 The verification must be done manually with the device control.
export-select-path = Select the path you want to export in the popup window...
export-starting = Starting export...
export-progress = Progress: {$progress}%
export-timeout = Export failed: timeout
export-canceled = Export canceled
export-labels-conflict = Labels conflict, what do you want to do?
export-aliases-conflict = Aliases conflict, what do you want to do?
common-copy = Copy
common-learn-more = Learn more
installer-descriptor-wrong-network = The descriptor is for another network
installer-descriptor-read-failed = Failed to read the descriptor
installer-import-backup = Import backup
installer-backup-imported = Backup successfully imported!
installer-import-wallet-title = Import the wallet
installer-import-wallet-rescan-help = If you are using a Bitcoin Core node, you will need to perform a rescan of the blockchain after creating the wallet in order to see your coins and past transactions. This can be done in Settings > Node.
installer-invalid-descriptor = Invalid descriptor
installer-generate-mnemonic = Generate a new mnemonic
installer-backup-mnemonic-warning = Beware to back up the mnemonic as it will NOT be stored on the computer.
installer-switch-account-help = Switch account if you already use the same hardware in other configurations
installer-import-xpub-device = Import an extended public key by selecting a signing device:
installer-share-xpubs-title = Share your public keys (Xpubs)
installer-no-device-connected = No signing device connected
installer-create-random-key = Or create a new random key:
installer-descriptor-template = Descriptor template
installer-the-descriptor = The descriptor
installer-register-descriptor-optional = This step is only necessary if you are using a signing device.
installer-register-descriptor-failed = Failed to register descriptor
installer-select-device-register = Select hardware wallet to register descriptor on:
installer-select-device-register-if-needed = If necessary, please select the signing device to register descriptor on:
installer-registered-descriptor-checkbox = I have registered the descriptor on my device(s)
installer-register-descriptor-title = Register descriptor
installer-back-up-descriptor = Back Up Descriptor
installer-backup-descriptor-title = Back Up your wallet configuration (Descriptor)
installer-export-backup-failed = Failed to export backup
installer-the-descriptor-label = The descriptor:
installer-backed-up-descriptor-checkbox = I have backed up my descriptor
installer-node-type = Node type:
installer-checking-connection = Checking connection...
installer-connection-checked = Connection checked
installer-check-connection = Check connection
installer-node-setup-title = Set up connection to the Bitcoin node
installer-enter-correct-address = Please enter correct address
installer-remote-bitcoin-node-warning = Connection to a remote Bitcoin node is not supported. Insert an IP address bound to the same machine running Liana (ignore this warning if that's already the case)
installer-rpc-auth = RPC authentication:
installer-cookie-path = Cookie path
installer-enter-correct-path = Please enter correct path
installer-user = User
installer-enter-correct-user = Please enter correct user
installer-password = Password
installer-enter-correct-password = Please enter correct password
installer-enter-correct-electrum-address = Please enter correct address (including port), optionally prefixed with tcp:// or ssl://
settings-cookie-file-path = Cookie file path
settings-valid-filesystem-path = Please enter a valid filesystem path
settings-valid-user = Please enter a valid user
settings-valid-password = Please enter a valid password
settings-socket-address = Socket address:
settings-valid-address = Please enter a valid address
settings-running = Running
settings-not-running = Not running
settings-blockchain-rescan = Blockchain rescan
settings-rescan-success = Successfully rescanned the blockchain
settings-rescanning = Rescanning...{$progress}%
settings-year = Year:
settings-month = Month:
settings-day = Day:
settings-date-invalid = Provided date is invalid
settings-date-before-prune = Provided date earlier than the node prune height
settings-date-future = Provided date is in the future
settings-start-rescan = Start rescan
settings-starting-rescan = Starting rescan...
settings-backup-encrypted-descriptor = Back up encrypted descriptor
settings-backup-encrypted-descriptor-tooltip = An encrypted descriptor file (.bed) you can store anywhere. To decrypt it, you need one of your signing devices or xpubs.
settings-wallet-descriptor = Wallet descriptor:
settings-register-on-device = Register on hardware device
settings-wallet-alias = Wallet alias:
settings-alias = Alias
settings-alias-too-long = Please enter alias that is not too long
settings-fingerprint-aliases = Fingerprint aliases:
settings-correct-alias = Please enter correct alias
settings-updated = Updated
settings-update = Update
settings-updating = Updating
common-and = and
common-blocks = blocks
policy-signatures = { $count ->
    [one] 1 signature
   *[other] {$count} signatures
}
policy-out-of-by = out of {$count} by
policy-by = by
policy-primary-path = can always spend this wallet's funds (Primary path)
policy-inactive-for = can spend coins inactive for
policy-safety-net-path = (Safety Net path)
policy-recovery-path = (Recovery path #{$number})
policy-wallet-policy = The wallet policy:
settings-select-device = Select device:
common-skip = Skip
common-email = Email
common-continue = Continue
installer-backed-up-mnemonic-show-xpub = I have backed up the mnemonic, show the extended public key
installer-bitcoin-node-management = Bitcoin node management
installer-already-have-node = I already have a node
installer-auto-install-node = I want Liana to automatically install a Bitcoin node on my device
installer-existing-node-description = Select this option if you already have a Bitcoin node running locally or remotely. Liana will connect to it.
installer-managed-node-description = Liana will install a pruned node on your computer. You won't need to do anything except have some disk space available (~30GB required on mainnet) and wait for the initial synchronization with the network (it can take some days depending on your internet connection speed).
installer-start-bitcoin-node = Start Bitcoin full node
installer-download-complete = Download complete
installer-downloading-bitcoin-core = Downloading Bitcoin Core {$version}
installer-download-failed = Download failed: '{$error}'.
installer-installing-bitcoind = Installing bitcoind...
installer-installation-complete = Installation complete
installer-installation-failed = Installation failed: '{$error}'.
installer-bitcoind-already-installed = Liana-managed bitcoind already installed
installer-started = Started
installer-starting = Starting...
installer-finalize-installation = Finalize installation
installer-installing = Installing...
installer-installed = Installed
installer-threshold-keys = {$threshold} out of {$total} keys
installer-available-after-inactivity = Available after inactivity of ~
installer-able-to-move-any-time = Able to move the funds at any time.
installer-backup-mnemonic-title = Back Up your mnemonic
installer-backed-up-mnemonic-checkbox = I have backed up my mnemonic
installer-import-mnemonic-title = Import Mnemonic
installer-import-mnemonic = Import mnemonic
installer-choose-backend = Choose backend
installer-use-own-node = Use your own node
installer-use-liana-connect = Use Liana Connect
installer-local-wallet-description = Use your already existing Bitcoin node or automatically install one. The Liana wallet will not connect to any external server.

    This is the most private option, but the data is locally stored on this computer, only. You must perform your own backups, and share the descriptor with other people you want to be able to access the wallet.
installer-remote-backend-description = Use our service to instantly be ready to transact. Wizardsardine runs the infrastructure, allowing multiple computers or participants to connect and synchronize.

    This is a simpler and safer option for people who want Wizardsardine to keep a backup of their descriptor. You are still in control of your keys, and Wizardsardine does not have any control over your funds, but it will be able to see your wallet's information, associated to an email address. Privacy focused users should run their own infrastructure instead.
installer-more-backend-node-info = More information about backend and node options
installer-choose-existing-account = Choose an account you are already using:
installer-enter-wallet-email = Enter an email you want to associate with the wallet:
installer-enter-new-wallet-email = Or enter a new email you want to associate with the wallet:
installer-send-token = Send token
installer-auth-token-emailed = An authentication token has been emailed to you
installer-change-email = Change Email
installer-give-wallet-alias = Give your wallet an alias
installer-wallet-alias = Wallet alias
installer-change-alias-later = You will be able to change it later in Settings > Wallet
common-edit = Edit
common-set = Set
common-apply = Apply
common-replace = Replace
common-retry = Retry
installer-descriptor-type = Descriptor type
installer-taproot-supported-version = Taproot is only supported by Liana version 5.0 and above
installer-add-safety-net-key = Add Safety Net key
installer-add-key = Add key
installer-keys-inactivity = Keys can move the funds after inactivity of:
installer-sequence-value-warning = Value must be greater than 0 and lower than 65535
installer-threshold = Threshold:
installer-key-name-alias = Key name (alias):
installer-key-name-help = Give this key a friendly name. It will help you identify it later:
installer-key-alias-placeholder = E.g. My Hardware Wallet
installer-key-path-account = Key path account:
installer-key-index = Key @{$index}:
decrypt-unlock-device = Please unlock or open app on the device
decrypt-try-device = Try to decrypt with this device...
decrypt-device-failed = Failed to decrypt file with this device
decrypt-device-description = Plug in and unlock a hardware device belonging to this setup to automatically decrypt the backup
decrypt-other-options = Other options
decrypt-airgap-help = Using an air-gapped device? Export the xpub from your device, then use the upload or paste option. If you don't know the correct derivation path, try with the following:
decrypt-provide-xpub = Provide one of the xpubs used in this wallet.
decrypt-upload-xpub-file = Upload extended public key file
decrypt-pairing-code = Pairing code: {$code}
decrypt-paste-xpub = Paste an extended public key
decrypt-enter-mnemonic-unsafe = UNSAFE: Enter mnemonic of one of the keys
decrypt-enter-mnemonic-warning = This option is not secure. I understand that entering a mnemonic on a computer may result in theft of my funds.
decrypt-backup-file = Decrypt backup file
decrypt-invalid-encoding = The file cannot be decoded properly, it seems not to be an encrypted backup.
decrypt-invalid-type = The file has been decrypted but the content type is not supported.
decrypt-invalid-descriptor = The file has been decrypted but the descriptor is not a valid Liana descriptor.
installer-introduction = Introduction
installer-build-your-own = Build your own
installer-custom-template-description-1 = For this setup you will need to define your primary and recovery spending policies. For security reasons, we suggest you use a separate Hardware Wallet for each key belonging to them.
installer-custom-template-description-2 = The keys belonging to your primary policy can always spend. Those belonging to the recovery policies will be able to spend only after a defined time of wallet inactivity, allowing for secure recovery and advanced spending policies.
installer-primary-spending-option = Primary spending option:
installer-primary-key = Primary key
installer-recovery-option = Recovery option #{$number}:
installer-recovery-key = Recovery key
installer-add-recovery-option = Add recovery option
installer-add-safety-net = Add Safety Net
installer-safety-net-description = This adds a final recovery option containing keys from professional key agents.

    Use this option if you have been provided one or more Safety Net tokens.
installer-safety-net = Safety Net:
installer-safety-net-key = Safety Net key
installer-set-keys = Set keys
installer-plug-hardware-device = Plug in a hardware device ...
installer-detected-hardware = Detected hardware
installer-no-other-sources = - No other sources detected -
installer-already-used-sources = Already used sources
installer-advanced-settings = Advanced settings
common-clear-all = Clear All
installer-customize = Customize
installer-choose-wallet-type = Choose wallet type
installer-simple-inheritance = Simple inheritance
installer-simple-inheritance-description = Two keys required, one for yourself to spend and another for your heir.
installer-expanding-multisig = Expanding multisig
installer-expanding-multisig-description = Two keys required to spend, with an extra key as a backup.
installer-build-your-own-description = Create a custom setup that fits all your needs.
installer-simple-inheritance-wallet = Simple inheritance wallet
installer-inheritance-description-1 = For this setup you will need 2 Keys: Your Primary Key (for yourself) and an Inheritance Key (for your heir). For security reasons, we suggest you use a separate Hardware Wallet for each key.
installer-inheritance-key = Inheritance key
installer-inheritance-description-2 = You will always be able to spend using your Primary Key. After a period of inactivity (but not before that) your Inheritance Key will become able to recover your funds.
installer-device-no-taproot = This device does not support Taproot
installer-expanding-multisig-wallet = Expanding multisig wallet
installer-multisig-description-1 = For this setup you will need 3 keys: two Primary Keys and a Recovery Key. For security reasons, we suggest you use a separate Hardware Wallet for each key.
installer-primary-key-number = Primary key #{$number}
installer-multisig-description-2 = The Primary Keys will compose a 2-of-2 multisig which will always be able to spend. In case one of your keys becomes unavailable, after a period of inactivity you will be able to recover your funds using the Recovery Key together with one of your Primary Keys (2-of-3 multisig):
installer-key-source-no-taproot = This key source does not support Taproot
common-update = Update
psbt-transaction-saved = Transaction is saved
psbt-save-transaction = Save this transaction
psbt-transaction-broadcast = Transaction is broadcast
psbt-broadcast-transaction = Broadcast the transaction
psbt-broadcast = Broadcast
psbt-broadcast-invalidates-some = WARNING: Broadcasting this transaction will invalidate some pending payments.
psbt-broadcast-invalidates-one = WARNING: Broadcasting this transaction will invalidate a pending payment.
psbt-broadcast-conflicts-some = The following transactions are spending one or more inputs from the transaction to be broadcast and will be dropped, along with any other transactions that depend on them:
psbt-broadcast-conflicts-one = The following transaction is spending one or more inputs from the transaction to be broadcast and will be dropped, along with any other transactions that depend on it:
psbt-delete-success = Successfully deleted this transaction.
psbt-go-back = Go back to PSBTs
psbt-delete-this = Delete this PSBT
psbt-missing-inputs = Missing information about transaction inputs
psbt-sign-save-before-export = Sign or save the transaction first to enable export
psbt-sign = Sign
psbt-status = Status
psbt-ready = Ready
psbt-signed-by = signed by
psbt-not-ready = Not ready
psbt-finalizing-requires = Finalizing this transaction requires:
psbt-more-signatures = { $count ->
    [0] no more signatures
    [one] 1 more signature from
   *[other] {$count} more signatures from
}
psbt-already-signed-by = , already signed by
psbt-coins-spent = { $count ->
    [one] 1 coin spent
   *[other] {$count} coins spent
}
psbt-payments = { $count ->
    [one] 1 payment
   *[other] {$count} payments
}
psbt-no-payment = 0 payment
psbt-change = Change
psbt-select-signing-device = Select signing device to sign with:
psbt-device-sign-failed = Device failed to sign
psbt-label = PSBT:
psbt-insert-updated = Insert updated PSBT:
psbt-base64-correct-warning = Please enter the correct base64 encoded PSBT
psbt-spend-updated = Spend transaction is updated
common-back = Back
payment-outgoing = Outgoing payment
payment-incoming = Incoming payment
payment-title = Payment
payment-see-transaction-details = See transaction details
label-add = Add label
label-label = Label
label-invalid-length = Invalid label length, cannot be greater than 100
loader-starting-daemon = Starting daemon...
loader-connecting-daemon = Connecting to daemon...
loader-progress = Progress {$progress}%
loader-sync-progress-1 = Bitcoin Core is synchronising the blockchain. A full synchronisation typically takes a few days and is resource-intensive. Once the initial synchronisation is done, the next ones will be much faster.
loader-sync-progress-2 = Bitcoin Core is synchronising the blockchain. This will take a while, depending on the last time it was done, your internet connection, and your computer performance.
loader-sync-progress-3 = Bitcoin Core is synchronising the blockchain. This may take a few minutes, depending on the last time it was done, your internet connection, and your computer performance.
loader-failed-bitcoind = Liana failed to start, please check if bitcoind is running
loader-failed = Liana failed to start
business-common-you = You
business-edited-by =  by {$name}
business-edited-relative = Edited{$editor} {$time}
business-login-email-help = Enter the email associated with your account
business-select-account = Select an account to continue
business-connect-another-email = Connect with another email
installer-auth-token-emailed-to = An authentication token has been emailed to
business-wallet-count = { $count ->
    [one] (1 wallet)
   *[other] ({$count} wallets)
}
business-key-count = { $count ->
    [one] (1 key)
   *[other] ({$count} keys)
}
business-contact-create-account = Contact WizardSardine to create an account.
business-no-orgs-search = No organizations found matching your search.
business-organizations = Organizations
business-select-organization = Select an Organization
business-filter-organizations = Filter organizations...
business-organization = Organization
business-wallet = Wallet
business-wallets = Wallets
business-select-wallet = Select wallet
business-create-wallet = Create a wallet
business-filter-wallets = Filter wallets...
business-no-wallets-search = No wallets found matching your search.
business-role-admin = Admin
business-role-manager = Manager
business-role-participant = Participant
business-keys = Keys
business-keys-instruction = Add the keys that will be part of this wallet and link each one to its owner's email address.
business-add-key = + Add a key
business-unable-load-wallet = Unable to load wallet
business-service-unavailable = The service is temporarily unavailable. Your wallet data and funds are not affected.
business-try-again-support = Please try again shortly. If the issue persists, contact support.
business-loading-wallet = Loading wallet...
business-manage-keys = Manage Keys
business-send-for-approval = Send for approval
business-unlock = Unlock
business-approve-template = Approve Template
business-template = Template
business-set-keys = Set Keys
business-your-key-set = Your key is set.
business-your-keys-set = Your keys are set.
business-wait-other-key-setup = Once the other participants complete their key setup, you'll be able to access the wallet.
business-xpub-instruction = Select a key to complete its setup. Keys can be set up by each key manager individually, or by the wallet manager on their behalf. You can connect a hardware device (recommended) or manually add an extended public key (xpub).
business-wallet-set-keys = {$wallet} - Set Keys
business-no-keys-assigned = No keys assigned to you
business-no-keys-found = No keys found
business-your-keys = Your keys:
business-other-participants-keys = Other participants' keys:
business-register-devices = Register Devices
business-register-wallet-devices = Register Wallet on Devices
business-register-wallet-devices-help = Register the wallet descriptor on each device, or skip if unavailable.
business-no-devices-register = No devices to register
business-no-devices-assigned = You don't have any devices assigned in this wallet.
business-register = Register
business-device-unsupported-locked = Device not supported or locked
business-connect-device-register = Connect the associated device to register
business-xpub-already-set-help = This key already has an xpub. You can replace it by fetching from a device, importing from file, or pasting. Use the Clear button to remove it completely.
business-current-xpub = Current xpub:
business-select-key-source = Select key source - {$alias}
business-fetching-device = Fetching from device...
business-account-number = Account #{$index}
business-no-hardware-wallets = No hardware wallets detected. Connect a device and unlock it.
business-detected-devices = Detected Devices:
business-unlock-device = Please unlock the device
business-not-part-wallet = Not part of this wallet (#{$fingerprint})
business-wrong-network-device = Wrong network in device settings
business-device-version-unsupported = Device version not supported, upgrade to version > {$version}
business-unsupported-method = Unsupported method: {$method}
business-open-app-device = Please open the app on device
business-import-xpub-file = Import extended public key file
business-edit-primary-path = Edit Primary Path
business-edit-recovery-path = Edit Recovery Path
business-create-new-path = Create New Path
business-keys-in-path = Keys in Path:
business-no-keys-available = No keys available. Add keys first.
business-key-number = Key {$id}
business-invalid-threshold = Invalid threshold value
business-threshold-range = Threshold (1-{$count}):
business-timelock-zero = Timelock cannot be zero
business-max-unit = Max {$max} {$unit}
business-duplicate-timelock = Duplicate timelock
business-timelock = Timelock:
business-max-unit-label = Max: {$max} {$unit}
business-no-timelock = No timelock
business-after-months = { $count ->
    [one] After 1 month
   *[other] After {$count} months
}
business-after-days = { $count ->
    [one] After 1 day
   *[other] After {$count} days
}
business-after-hours = { $count ->
    [one] After 1 hour
   *[other] After {$count} hours
}
business-no-keys = No keys
business-all-of = All of {$names}
business-threshold-of = {$threshold} of {$names}
business-spendable-anytime = Spendable anytime
business-add-recovery-path = + Add a recovery path
business-confirm-device = Please confirm on your device...
business-registering-wallet = Registering Wallet
business-registration-failed = Registration Failed
business-confirm-coldcard-success = Please confirm on your Coldcard that the wallet registration completed successfully.
business-did-registration-succeed = Did the registration succeed on your Coldcard?
business-confirm-registration = Confirm Registration
business-keep-my-changes = Keep my changes
common-reload = Reload
business-new-key = New Key
business-edit-key = Edit Key
business-key-alias = Key Alias
business-enter-key-alias = Enter key alias
business-key-type = Key Type
business-key-type-tooltip = Internal: keys held by your organization.
    External: keys held by third parties.
    Cosigner: Professional third party co-signing key.
    SafetyNet: Professional third party recovery key.
business-key-manager-email = Email Address of the Key Manager
business-enter-email-address = Enter email address
business-enter-token-placeholder = Enter token (e.g., 42-absent-cake-eagle)
business-authenticated = Authenticated
business-connection-failed = Connection failed
business-user-session-not-found = User session not found. Please log in again or contact WizardSardine.
business-access-error = Access Error
business-wallet-access-denied = You do not have access to this wallet. Contact WizardSardine.
business-backend-error = Backend error
business-connection-error = Connection Error
business-lost-connection-restart = Lost connection to the server. Please restart the application.
business-account-connection-failed = Failed to connect with account {$email}. The session may have expired.
business-key-deleted = Key Deleted
business-key-deleted-message = The key you were editing was deleted by another user.
business-key-modified = Key Modified
business-key-modified-message = This key was modified by another user. Would you like to reload the server version or keep your changes?
business-key-removed = Key Removed
business-key-removed-from-path = "{$alias}" was deleted by another user and has been removed from your path selection.
business-path-modified = Path Modified
business-primary-path-modified-message = The primary path was modified by another user. Would you like to reload the server version or keep your changes?
business-path-deleted = Path Deleted
business-path-deleted-message = The path you were editing was deleted by another user.
business-recovery-path-modified-message = This recovery path was modified by another user. Would you like to reload the server version or keep your changes?
business-device-locked-unlock = Device is locked. Please unlock it first.
business-device-not-supported = Device is not supported
business-hardware-wallet-not-found = Hardware wallet not found
business-select-xpub-file = Select xpub file
business-text-files = Text files
business-all-files = All files
business-file-read-failed = Failed to read file: {$error}
business-file-dialog-result-failed = Failed to receive result from file dialog thread
business-clipboard-empty = Clipboard is empty
business-no-descriptor-available = No descriptor available
business-no-wallet-selected = No wallet selected
business-no-user-id-available = No user ID available
business-auth-code-request-failed = Failed to request authentication code from server.
business-login-failed = Login failed.
business-xpub-empty = Extended public key cannot be empty.
business-xpub-invalid-format = Invalid extended public key format: {$error}
business-xpub-invalid-network = Extended public key is not valid for {$network}.
business-device-disconnected = Device disconnected
business-token-invalid = Invalid token.
business-token-duplicate = Duplicate token.
business-code-six-digits = Code must contain only 6 digits.
business-admin-name = Admin{$name}
time-just-now = just now
time-minutes-ago = { $count ->
    [one] 1 minute ago
   *[other] {$count} minutes ago
}
time-hours-ago = { $count ->
    [one] 1 hour ago
   *[other] {$count} hours ago
}
time-days-ago = { $count ->
    [one] 1 day ago
   *[other] {$count} days ago
}
time-weeks-ago = { $count ->
    [one] 1 week ago
   *[other] {$count} weeks ago
}
time-months-ago = { $count ->
    [one] 1 month ago
   *[other] {$count} months ago
}
error-unknown = Unknown error
warning-wallet-error = Wallet error
warning-fields-invalid = Some fields are invalid
warning-internal-error = Internal error
warning-http-code-error = HTTP error {$code}: {$error}
warning-http-error = HTTP error: {$error}
warning-daemon-start-failed = Daemon failed to start
warning-daemon-client-unsupported = Daemon client is not supported
warning-daemon-communication-failed = Communication with Daemon failed
warning-daemon-stopped = Daemon stopped
warning-coin-selection-error = Error when selecting coins for spend
warning-backend-feature-unimplemented = Feature not implemented for this backend
warning-hardware-wallet-error = Hardware wallet error
warning-descriptor-analysis-error = Descriptor analysis error: '{$error}'.
warning-spend-creation-error = Spend creation error: '{$error}'.
warning-restore-backup-failed = Failed to restore backup: {$error}
warning-fiat-price-error = Fiat price error: {$error}
common-ok = OK
common-yes = Yes
common-no = No
common-reset-timelock = Reset timelock
common-go-to-rescan = Go to rescan
common-dismiss = Dismiss
pill-recovery = Recovery
pill-recovery-tooltip = This transaction is using a recovery path
pill-batch = Batch
pill-batch-tooltip = This transaction contains multiple payments
pill-deprecated = Deprecated
pill-deprecated-tooltip = This transaction cannot be included in the blockchain anymore.
pill-spent = Spent
pill-spent-tooltip = The transaction was included in the blockchain.
pill-unsigned = Unsigned
pill-unsigned-tooltip = This transaction is missing signature(s)
pill-signed = To broadcast
pill-signed-tooltip = This transaction is signed & ready to broadcast
pill-unconfirmed = Unconfirmed
pill-unconfirmed-tooltip = Do not treat this as a payment until it is confirmed
pill-confirmed = Confirmed
pill-confirmed-tooltip = This transaction has been included in a block
pill-key-internal = Internal
pill-key-internal-tooltip = Key held by your organization
pill-key-external = External
pill-key-external-tooltip = Key held by third parties
pill-key-cosigner = Cosigner
pill-key-cosigner-tooltip = Professional third party co-signing key
pill-key-safety-net = Safety Net
pill-key-safety-net-tooltip = Professional third party recovery key
pill-to-approve = To approve
pill-draft = Draft
pill-set-keys = Set keys
pill-active = Active
pill-ws-admin = WS Admin
pill-register = Register
pill-xpub-set = ✓ Set
pill-xpub-not-set = Not Set
pill-rescan-progress = Rescan… {$progress}%
pill-available = Available
pill-today = Today
pill-recovery-available-tooltip = Recovery option(s) already available
pill-first-recovery-today = First recovery option available today
pill-first-recovery-in = First recovery option available in {$units}
duration-years = { $count ->
    [one] 1 year
   *[other] {$count} years
}
duration-months = { $count ->
    [one] 1 month
   *[other] {$count} months
}
duration-days = { $count ->
    [one] 1 day
   *[other] {$count} days
}
duration-days-approx = ~{$count} days
duration-hours = { $count ->
    [one] 1 hour
   *[other] {$count} hours
}
duration-minutes = { $count ->
    [one] 1 minute
   *[other] {$count} minutes
}
