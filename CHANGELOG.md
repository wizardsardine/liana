# Liana daemon and GUI release notes

## 4.0

This release introduces support for bumping the fees of a transaction, verifying a deposit address
on your signing device, and more.

### Features

- The `outpoints` parameter of the `createspend` command is now optional. If not provided, we'll
  select coins automatically.
- A new `listaddresses` command was introduced.
- A new `rbfpsbt` command was introduced.
- The `createspend` command has a new, optional, `change_address` parameter. This makes it possible
  to create a transaction which sweeps all funds from the wallet.

#### GUI-specific

- When creating a Spend transaction, coins to be spent are now pre-selected. The selection is
  updated as you update the recipients and/or the feerate. The selection will stop being modified if
  you change it manually.
- It is now possible to verify deposit addresses on your signing device.
- You can now bump the fees of an unconfirmed transaction. A "bump fee" button was introduced in the
  transaction details (available from the list of transactions).
- You can now "cancel" an unconfirmed transaction. A "cancel" button was introduced in the
  transaction details (available from the list of transactions). NOTE: the cancel feature is not
  guaranteed to work. It's simply leveraging RBF to double spend the outgoing transaction with a
  transaction paying back to ourselves.
- You can now delete a wallet for a specific network from the launcher.
- When selecting a signing device, those which are not related to the wallet or which don't support
  a specific method (such as displaying an address) are now greyed-out.
- The managed Bitcoin Core version was bumped to 26.0.

### Fixes

- In case a transaction spending one of our coins was RBF'd, we could incorrectly assign an
  incorrect spending transaction to this coin.

#### GUI-specific

- Setting a feerate larger than `2^64` when creating a Spend would previously crash the software.
- When displaying a PSBT the software could crash if some of the inputs of the transaction
  disappeared (were double-spent).

## 3.0

This release introduces support for the BitBox02 signing device, as well as the possibility to label
payments, batches of payments, coins and addresses.

### Breaking changes

- Descriptors with duplicate signers **within a single spending path** are not supported anymore.
  Note re-using a signer across spending paths (for instance for a decaying multisig) is still
  supported, this only concerns pathological descriptor where the same signer would be repeated in
  the same path.

### Features

- Two new optional parameters were introduced to the `listcoins` command to be able to filter coins
  by status (confirmed, spent, etc..) and outpoints (to query specific coins).
- We updated the ["quick try"](https://github.com/wizardsardine/liana/blob/master/doc/TRY.md) guide
  to make use of the managed `bitcoind`. Trying out Liana on Signet is now easier than ever!

#### GUI-specific

- You can now use the [BitBox02](https://bitbox.swiss/bitbox02/) signing device. The minimum
  supported version of the firmware is v9.15.0, so make sure to upgrade!
- It's now possible to label coins and payments (that is, a transactions output). It's also possible
  to label batches of payments (that is, a transaction itself) and addresses.
- The number of steps in the installer was reduced by dropping the final confirmation screen. The
  wallet will now start automatically after configuration, reducing the information load and number
  of clicks for the user.
- All text inputs are now sanitized to remove whitespaces. No need to manually remove a trailing
  whitespace when importing a PSBT for instance!
- Various loading screens at startup were updated to include more information.
- The transaction fee rate is now displayed in addition to the absolute fee in the details.
- The managed bitcoind version was bumped to 25.1 for new installations.

### Fixes

- We fixed the minimum glibc version in the dependencies of the Debian package.
- We could previously crash if we were started up against a bitcoind itself recovering from a previous
  crash and in the process of re-connecting the entire chain.
- A few incorrect commands were corrected in the wallet recovery documentation.

#### GUI-specific

- Selecting a coinbase transaction won't make the GUI crash anymore.
- At startup there could previously be a small lag before the home page gets updated with the list
  of payments and correct balance.
- During installation, when using a managed bitcoind, the GUI could previously freeze after starting
  up bitcoind.

## 2.0

This release significantly simplifies the installation of Liana. It also fixes a number of small
bugs and glitches.

### Breaking changes

- Descriptors created with Liana v2 may not be backward compatible with Liana v1.

### Features

- We made it possible to re-use an xpub within a descriptor, so long as it uses a different
  derivation path.
- We added new RPC commands allowing for updating and querying labels of txids, addresses and
  outpoints.
- We've made our RPC connection to `bitcoind` more robust.
- We now distribute packages for Debian-based and Arch-based Linux distributions.
- A guide on how to recover a Liana wallet backup with Bitcoin Core was added.
- An example config file for running `lianad` was added.
- We've worked around the use of prefixed-paths on Windows thereby allowing us to bring back the
  watchonly wallet at the same location as for other operating systems, under our own datadir.

#### GUI-specific

- The UX for creating a descriptor where a signer is present in different spending paths was
  significantly improved.
- The installation process, as well as the usage of the wallet, was made more user friendly by
  optionally encapsulating the management (download, start and stop) of the `bitcoind`.

### Fixes

- We more gracefully stop the Bitcoin backend poller when the block chain is still in the process of
  being synchronized. This would previously appear to hang and could freeze the GUI.
- The handling of conflicting unconfirmed spend transactions (RBFs) was fixed.
- If paid directly through a coinbase transaction output, the wallet could have previously missed
  it. This was fixed.
- We now correctly treat immature coinbase deposits as unspendable. They are otherwise treated
  similarly to unconfirmed coins.
- We now tell `bitcoind` to load our watchonly wallet upon startup. Not loading the watchonly wallet
  on startup could make `bitcoind` unable to load it without reindexing the block chain, if using an
  aggressive pruning configuration.
- `lianad` will now print a more helpful message on startup failure (and link to the newly added
  config file example).

#### GUI-specific

- We now check the network of xpubs when importing a descriptor.
- The GUI would sometimes fail to connect to the Specter DIY signing device.
- We now convey the descriptor registration step on a signing device isn't necessary if none were
  used.
- We could previously appear to hang during shutdown.


## 1.0

This is the first non-beta release of Liana.

Improvements were concentrated on the GUI. The UI was entirely overhauled.

### Features

- The `createspend` command now allows you to not provide any destination. In this case it will
  create a send-to-self transaction containing a single change output.

#### GUI-specific

- Overall there is a new layout and color scheme. The "draft transactions" menu was renamed to
  the more common "PSBTs".
- The homepage now features a list of payments, instead of transactions.
- The spend transaction creation process is now contained in a single screen. It allows you to
  easily create a send-to-self transaction by not specifying any recipient.
- The homepage will now feature an approximation of the remaining time before the first recovery
  path becomes available.
- The homepage now features a button to refresh all coins whose recovery path is available (or close
  to be), if there is any.
- Entries in the coins list now features a button to refresh a coin (create a send-to-self
  transaction in order to restart the timelock).
- You can now generate multiple receive addresses in a row.
- We now display the alias of signing devices (if any) in the final installer step.

### Fixes

#### GUI-specific

- Send-to-self transactions are now displayed as such instead of being affected a "0.00000BTC"
  value.
- The installer will not present a step to register the descriptor on the signing device if there
  isn't any.
- Some wording improvements all around.
- The slider to configure timelocks in the installer now has a step of 144 (instead of 1).

## 0.4

This fourth release brings support for descriptors with multiple recovery path as well as several
usability improvements in the GUI around signing devices management, and more.

### Features

- We now support having multiple recovery path in a descriptor.
- We now support more general descriptors: multisigs in the primary or any of the recovery paths
  henceforth aren't required to use `multi()` anymore and the maximum number of keys per spending
  path is thereby lifted.

#### GUI-specific

- You can now re-register the descriptor on a hardware signing device in the settings.
- You can now change the alias of each of the signers from the settings panel.
- At signing time we now warn you if the descriptor is not registered on the signing device.
- The signer alias is now displayed along with its type when signing.
- You can now connect to a running daemon without having to provide a path to its configuration,
  provided it is using the default location for its data directory (or `--datadir` is used).
- The GUI will now log to a `installer.log` file at the root of the datadir during installation, and
  to a `<network>/liana-gui.log` when running. In case of crash, this will contain a backtrace.
- During installation we now check the connection to bitcoind.

### Fixes

- We won't error when parsing of descriptor with a 1-of-N multisig as primary path.
- We won't error at startup if our watchonly wallet is loading on bitcoind. Instead, we'll wait for
  completion of the previous loading attempt.

#### GUI-specific

- Blank addresses aren't treated as duplicates when creating a transaction.

## 0.3.1

A patch release for a serious bug fix in the GUI installer.

### Fixes

#### GUI-specific

- Under very specific conditions the GUI installer would not store the mnemonic words corresponding
  to a hot key that was used in the descriptor, nor present it to the user for backup.

## 0.3

A small release which brings some fixes as well as the possibility to use Liana as a "hot wallet".

### Features

- Hot keys: users can now generate and sign with keys that are stored on the device. It is
  recommended to be only used for testing for now.

#### GUI-specific

- It is now possible to use multiple signing device of the same type without having to first connect
  one then the other.

### Fixes

- When used as a daemon the `lianad` process had its PID and logs file mixed up. This is now fixed.
- We fixed the transaction creation sanity check that was overestimating the transaction fee.

#### GUI-specific

- In the installer flow, extended keys are now shared without the `/<0;1>/*` suffix.

## 0.2

The second release of Liana brings various fixes as well as the possibility to use a multisig in
either, or both, of the spending paths.

### Features

- Multisig: we now support descriptors with multiple keys both in the primary (non-timelocked)
  spending path and the recovery (timelocked) path.

#### GUI-specific

- You can now import and update Spend transaction drafts as PSBTs to collaboratively create and sign
  transactions.
- When creating a new descriptor you can now set an alias for each key. Those will be displayed when
  inspecting a transaction's signatories.
- Amounts are now displayed with the sats in bold for better redability.

### Fixes

- We now remove the fixed interpreter and rpath set by GUIX reproducible builds in the `liana-cli`
  ELF binary.
- We now check the `bitcoind` version before trying to import a Miniscript descriptor.
- We now discard unconfirmed incoming payments that were dropped from our mempool.
- **Breaking change**: the first version of Liana mistakenly accepted extended keys without origin
  in descriptors. This meant that unless this extended key was the master extended key of a chain,
  it would not be possible to sign with it (since signing devices need to know the origin). Starting
  from version 2 Liana forces extended keys to contain an origin (of the form `[a1b2c3d4]`) to avoid
  this footgun. This means that existing descriptors might have to be migrated, but it's very likely
  only for test configurations where an xpub wasn't gathered from a signing device (which prepends
  an origin) but generated (probably imported from Coleman's website) and pasted without origin.

#### GUI-specific

- Various wording fixes on the UI.
- Amounts are now updated when moving between steps in the Spend creation flow.
- Coins are now sorted by age when displayed as a list.
- Some flakiness in the connection to a signing device were fixed.
- The descriptor registration on a signing device step in the installer was made clearer.
