# Recover your Liana wallet using Bitcoin Core

This document walks you through the step of recovering a Liana wallet using `bitcoind` and
`bitcoin-cli`. You will need a couple things in order to follow this guide:
- Your Liana wallet descriptor.
- The date at which you created the Liana wallet (approximately).
- The necessary signing devices. In this example we'll use a Ledger Nano S and [HWI](https://github.com/bitcoin-core/HWI).
- An address to sweep the funds to. Here we'll use `tb1qed7lyessqcecav5uxultf6zc8nefd9kalgaa7dwrglcc6ld5vd3qe20spe`.
- An installed and synchronized `bitcoind`. If you don't have Bitcoin Core yet you can download it
  [here](https://bitcoincore.org/en/download/). The minimum supported version for this guide is
  25.0.

This document **is not** about the similarly named "timelocked recovery path" feature. You can spend
your coins through (one of) the timelocked recovery path(s) using Bitcoin Core too, but this
document is about the instructions for recovering your coins using the Bitcoin Core wallet in the
improbable case you cannot access the Liana wallet for whatever reason.

Here's a list of the steps we'll undertake:
1. Create a dedicated watchonly wallet on Bitcoin Core, and import the Liana wallet descriptor there.
2. Scan the blockchain for your coins since the birthdate of your wallet.
3. Create a transaction that sweeps all your coins to the prepared address.
4. Sign this transaction using the appropriate devices.
5. Broadcast the transaction.

NB: this guide was written with an example on Linux. It should be trivial to follow on any other
operating system though, with only minor adaptations needed to the examples.

## Create a Bitcoin Core wallet and import the Liana descriptor

Your Liana wallet descriptor is probably of the form:
```
wsh(or_d(pk([a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh/<0;1>/\*),and_v(v:pkh([c477fd13/48'/1'/0'/2']tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH/<0;1>/\*),older(3))))#8ldsjayd
```
Note the curious `<0;1>` step in the xpubs' derivation path. It's called a
[multipath](https://github.com/bitcoin/bips/blob/master/bip-0389.mediawiki), a way to compress two
derivation steps in a single expression so you only have to backup a single descriptor instead of
two (one for deriving receive addresses and the other for change addresses).

### Step 1 : Split descriptors

If you're using Bitcoin Core 29.0 or above, you can skip to step 2 — multipath descriptors are supported and don't need 
to be split.

Otherwise it means that your node does not support multipath descriptors. So we'll have to split the descriptor
in two: one for receive and one for change. To do so:
- Make two copies of your descriptor, without the checksum (the part following the `#`).
- For each of them walk through the xpubs.
- Replace each multipath expression with the first index (here `0`) for the first copy and the
  second index for the second copy (here `1`).

For our example descriptor above this gives us:
```
wsh(or_d(pk([a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh/0/*),and_v(v:pkh([c477fd13/48'/1'/0'/2']tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH/0/*),older(3))))
```
and
```
wsh(or_d(pk([a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh/1/*),and_v(v:pkh([c477fd13/48'/1'/0'/2']tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH/1/*),older(3))))
```

We'll need a checksum for each in order to be able to import them on the Bitcoin Core wallet. For
this use the `getdescriptorinfo` command, gather the `checksum` field and append it to the
descriptor after a `#`. For instance with the first descriptor above:
```shell
bitcoin-cli getdescriptorinfo "wsh(or_d(pk([a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh/0/*),and_v(v:pkh([c477fd13/48'/1'/0'/2']tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH/0/*),older(3))))"
```
Output:
```
{
  "descriptor": "wsh(or_d(pk([a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh/0/*),and_v(v:pkh([c477fd13/48'/1'/0'/2']tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH/0/*),older(3))))#nhtumqkr",
  "checksum": "nhtumqkr",
  "isrange": true,
  "issolvable": true,
  "hasprivatekeys": false
}
```
So the resulting descriptor is
```
wsh(or_d(pk([a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh/0/*),and_v(v:pkh([c477fd13/48'/1'/0'/2']tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH/0/*),older(3))))#nhtumqkr
```

Make sure to do this for both descriptors.

### Step 2 : Create the wallet

Create the dedicated watchonly wallet on Bitcoin Core:
```shell
bitcoin-cli -signet createwallet liana_recovery true 
```
Output:
```
{
  "name": "liana_recovery"
}
```

We are now going to import the receive and change descriptors to the wallet. This process will take
care of rescanning the block chain for transactions involving these descriptors. Make sure you have
your wallet birthdate in a timestamp format (we'll use `1682920310`, May 1st 2023). In order to do
this we are going to use the `importdescriptors` command and pass it our two descriptors (with the
checksum appended) along with the wallet birthdate as a timestamp. Note this command may take a
while as it's going to be rescanning the block chain. The farther in the past the birthdate, the
longer it will take. If the command times out you will be able to inspect the progress using the
`getwalletinfo` command.

If you're using a version earlier than 29.0 :
```shell
bitcoin-cli -signet -rpcwallet=liana_recovery importdescriptors "[{\"desc\": \"wsh(or_d(pk([a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh/0/*),and_v(v:pkh([c477fd13/48'/1'/0'/2']tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH/0/*),older(3))))#nhtumqkr\", \"range\": [0,10000], \"timestamp\": 1682920310, \"active\": true, \"internal\":false}, {\"desc\": \"wsh(or_d(pk([a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh/1/*),and_v(v:pkh([c477fd13/48'/1'/0'/2']tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH/1/*),older(3))))#vpa5k5p6\", \"range\": [0,10000], \"timestamp\": 1682920310, \"active\": true, \"internal\":true}]"
```

Since 29.0 :
```shell
bitcoin-cli -signet -rpcwallet=liana_recovery importdescriptors "[{\"desc\":\"wsh(or_d(pk([a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh/<0;1>/*),and_v(v:pkh([c477fd13/48'/1'/0'/2']tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH/<0;1>/*),older(3))))#8ldsjayd\", \"range\": [0,10000], \"timestamp\": 1682920310, \"active\": true}]"
```

The output should look like this :
```
[
  {
    "success": true
  }
]
```
You should get 2 "success" in case you're using a version earlier than 29.0 because you imported 2 descriptors.

Alright! You should now be able to see your coins on the wallet. You can check your balance with the
`getbalance` command or list all the unspent coins using `listunspent`. For instance in our case:
```shell
$ bitcoin-cli -signet -rpcwallet=liana_recovery listunspent
[
  {
    "txid": "054166e08bd3e019031ec2e4f6a755913e924f218b6c0ab792c2fd4d911a1011",
    "vout": 1,
    "address": "tb1qhvrkp9qpzp2cmk7ct745zpkuw3ak45hwvczx5534x89ymg3ae0vsfwyd7j",
    "label": "",
    "witnessScript": "2103d040bf48b05dfa7234fe39240edc3d89cc909d65b4fd55338ffa162985d4c5c9ac736476a9142517bf2e04d18ae59db290617eb60183d4893f6188ad53b268",
    "scriptPubKey": "0020bb0760940110558ddbd85fab4106dc747b6ad2ee66046a523531ca4da23dcbd9",
    "amount": 0.00010000,
    "confirmations": 152,
    "spendable": true,
    "solvable": true,
    "desc": "wsh(or_d(pk([a5c6b76e/48'/1'/0'/2'/0/1]03d040bf48b05dfa7234fe39240edc3d89cc909d65b4fd55338ffa162985d4c5c9),and_v(v:pkh([c477fd13/48'/1'/0'/2'/0/1]02113af9bac303954c884a077e1840b98fe0b8713ccace07adcbacf322e8f72fbd),older(3))))#8vukwv7f",
    "parent_descs": [
      "wsh(or_d(pk([a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh/0/*),and_v(v:pkh([c477fd13/48'/1'/0'/2']tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH/0/*),older(3))))#nhtumqkr"
    ],
    "safe": true
  }
]
```

## Sweep the coins out of the wallet

### Craft the sweep transaction

We are now going to create a PSBT that spends all the coins in the wallet to the pre-defined
address. Note merging all your coins into one is usually a bad practice for privacy purpose.
You can also spend them one by one or in other way if you wish. Here we are simply going to present
the simplest way of recovering from disaster.

We'll use the `sendall` command to sweep all the funds of the wallet to the pre-defined address
using a specific feerate. We'll use `20` sats/vbyte but use anything that fits.
```shell 
bitcoin-cli -signet -rpcwallet=liana_recovery -named sendall recipients='["tb1qed7lyessqcecav5uxultf6zc8nefd9kalgaa7dwrglcc6ld5vd3qe20spe"]' fee_rate=20
```
Output:
```
{
  "psbt": "cHNidP8BAF4CAAAAATEgOFWAFkRN09sCK0V1ES9UyMiUQRJrb0AG4j1y0q++AQAAAAD9////ARg4DwAAAAAAIgAgy33yZhAGM46ynDc+tOhYPPKWlt36O981w0fxjX20Y2IAAAAAAAEAfQIAAAABnk7wzbybA7Z6kvlO7hFYl5x2BN3L0otR5M5udXzGN/oBAAAAAP7///8CrKE2EQAAAAAWABRnUU8zi4M3eMplKigSvOdhtGuSdUBCDwAAAAAAIgAgxBhOa53udP66VGgn+vs7sboB8aXyJj4WddV6PsUjJvHoZAIAAQErQEIPAAAAAAAiACDEGE5rne50/rpUaCf6+zuxugHxpfImPhZ11Xo+xSMm8QEFQSEDFPd+lcxYMUvbhW51TN6Fzc5LFtQVTYbZBUxuEEpHHoCsc2R2qRRX/S6WeWl7J1PJvN+73IONRKHJYYitU7JoIgYDFPd+lcxYMUvbhW51TN6Fzc5LFtQVTYbZBUxuEEpHHoAcpca3bjAAAIABAACAAAAAgAIAAIAAAAAAAAAAACIGA2u7xjrIFe7ShVfXgPOqQwzKtxYkFeV52rNpNyzHfrNdHMR3/RMwAACAAQAAgAAAAIACAACAAAAAAAAAAAAAAA==",
  "complete": false
}
```

### Sign the sweep transaction

This part may depend on the signing devices you used with Liana. In general there should be plenty
of documentation available for each signer about how, given a PSBT, to sign it and potentially pass
it on to the next signer. Here we'll detail the signing process with a Ledger Nano S(+).

#### Install HWI

You will need the [`HWI`](https://github.com/bitcoin-core/HWI) tool to interface with the Ledger.
However, Salvatore's wallet policies (required to sign for Miniscript descriptors) are at the time
of writing not yet merged so we'll have to use his branch.

Get the HWI-with-wallet-policy pull request code:
```shell
git clone https://github.com/bitcoin-core/HWI
cd HWI
git fetch origin pull/647/head
git checkout FETCH_HEAD
```

And install it in a virtual environment (you may have to install Python and `pip`):
```shell
python3 -m venv venv
. venv/bin/activate
pip install poetry
poetry install
```

#### Sign with the Ledger

Register the descriptor on the Ledger. It's using a specific syntax to register both descriptors at
once: the wallet policies. You'll once again have to process the descriptor:
1. Take the original descriptor (with the `<0;1>` multipaths derivation steps).
2. Replace all xpubs with an `@` followed by an index number (`@0` for the first, `@1` for the
   second, etc..).
3. Compile a list of the xpubs **in the same order**.
4. Replace all `/<0;1>/*` derivation steps by `/**`. NOTE: **do not** replace those if the multipath
   step is anything other than `/<0;1>/*`. For instance if it's `/<2;3>/*` **leave it intact**.

For instance in our case the result is this wallet policy:
```
wsh(or_d(pk(@0/**),and_v(v:pkh(@1/**),older(3))))
```
With the following list of xpubs:
```
["[a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh","tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH"]
```

We can finally register it on the Ledger:
```shell
python3 -m hwi --chain test --device-type ledger registerpolicy --policy "wsh(or_d(pk(@0/**),and_v(v:pkh(@1/**),older(3))))" --name "Liana" --keys "[\"[a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh\",\"tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH\"]"
```
Output
``` 
{"proof_of_registration": "34602db12b76200ea96386c4cd10b8b1d60b84e060d2d6120073c06367a9506d"}
```
(The resulting proof of registration will be different in your case.)

Then go ahead and sign the PSBT we've previously crafted:
```shell
python3 -m hwi --chain test --device-type ledger signtx cHNidP8BAF4CAAAAATEgOFWAFkRN09sCK0V1ES9UyMiUQRJrb0AG4j1y0q++AQAAAAD9////ARg4DwAAAAAAIgAgy33yZhAGM46ynDc+tOhYPPKWlt36O981w0fxjX20Y2IAAAAAAAEAfQIAAAABnk7wzbybA7Z6kvlO7hFYl5x2BN3L0otR5M5udXzGN/oBAAAAAP7///8CrKE2EQAAAAAWABRnUU8zi4M3eMplKigSvOdhtGuSdUBCDwAAAAAAIgAgxBhOa53udP66VGgn+vs7sboB8aXyJj4WddV6PsUjJvHoZAIAAQErQEIPAAAAAAAiACDEGE5rne50/rpUaCf6+zuxugHxpfImPhZ11Xo+xSMm8QEFQSEDFPd+lcxYMUvbhW51TN6Fzc5LFtQVTYbZBUxuEEpHHoCsc2R2qRRX/S6WeWl7J1PJvN+73IONRKHJYYitU7JoIgYDFPd+lcxYMUvbhW51TN6Fzc5LFtQVTYbZBUxuEEpHHoAcpca3bjAAAIABAACAAAAAgAIAAIAAAAAAAAAAACIGA2u7xjrIFe7ShVfXgPOqQwzKtxYkFeV52rNpNyzHfrNdHMR3/RMwAACAAQAAgAAAAIACAACAAAAAAAAAAAAAAA== --policy "wsh(or_d(pk(@0/**),and_v(v:pkh(@1/**),older(3))))" --name "Liana" --keys "[\"[a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh\",\"tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH\"]" --extra "{\"proof_of_registration\": \"34602db12b76200ea96386c4cd10b8b1d60b84e060d2d6120073c06367a9506d\"}"
```
Output:
``` 
{"psbt": "cHNidP8BAF4CAAAAATEgOFWAFkRN09sCK0V1ES9UyMiUQRJrb0AG4j1y0q++AQAAAAD9////ARg4DwAAAAAAIgAgy33yZhAGM46ynDc+tOhYPPKWlt36O981w0fxjX20Y2IAAAAAAAEAfQIAAAABnk7wzbybA7Z6kvlO7hFYl5x2BN3L0otR5M5udXzGN/oBAAAAAP7///8CrKE2EQAAAAAWABRnUU8zi4M3eMplKigSvOdhtGuSdUBCDwAAAAAAIgAgxBhOa53udP66VGgn+vs7sboB8aXyJj4WddV6PsUjJvHoZAIAAQErQEIPAAAAAAAiACDEGE5rne50/rpUaCf6+zuxugHxpfImPhZ11Xo+xSMm8SICAxT3fpXMWDFL24VudUzehc3OSxbUFU2G2QVMbhBKRx6ASDBFAiEA1h9wI6XI+3Jz61f3rrR9L/6guHbcG7/Dl0a2CmEhWlICIFcwZKqKWe96gUkIQctln6K9R9U4nHx3qJxrHUhmkuf/AQEFQSEDFPd+lcxYMUvbhW51TN6Fzc5LFtQVTYbZBUxuEEpHHoCsc2R2qRRX/S6WeWl7J1PJvN+73IONRKHJYYitU7JoIgYDFPd+lcxYMUvbhW51TN6Fzc5LFtQVTYbZBUxuEEpHHoAcpca3bjAAAIABAACAAAAAgAIAAIAAAAAAAAAAACIGA2u7xjrIFe7ShVfXgPOqQwzKtxYkFeV52rNpNyzHfrNdHMR3/RMwAACAAQAAgAAAAIACAACAAAAAAAAAAAAAAA==", "signed": true}
```

### Broadcast the sweep transaction

Once you've gotten enough signatures for the PSBT, you are ready to broadcast the sweep transaction.

First, finalize the PSBT:
```shell
bitcoin-cli -signet finalizepsbt cHNidP8BAF4CAAAAATEgOFWAFkRN09sCK0V1ES9UyMiUQRJrb0AG4j1y0q++AQAAAAD9////ARg4DwAAAAAAIgAgy33yZhAGM46ynDc+tOhYPPKWlt36O981w0fxjX20Y2IAAAAAAAEAfQIAAAABnk7wzbybA7Z6kvlO7hFYl5x2BN3L0otR5M5udXzGN/oBAAAAAP7///8CrKE2EQAAAAAWABRnUU8zi4M3eMplKigSvOdhtGuSdUBCDwAAAAAAIgAgxBhOa53udP66VGgn+vs7sboB8aXyJj4WddV6PsUjJvHoZAIAAQErQEIPAAAAAAAiACDEGE5rne50/rpUaCf6+zuxugHxpfImPhZ11Xo+xSMm8SICAxT3fpXMWDFL24VudUzehc3OSxbUFU2G2QVMbhBKRx6ASDBFAiEA1h9wI6XI+3Jz61f3rrR9L/6guHbcG7/Dl0a2CmEhWlICIFcwZKqKWe96gUkIQctln6K9R9U4nHx3qJxrHUhmkuf/AQEFQSEDFPd+lcxYMUvbhW51TN6Fzc5LFtQVTYbZBUxuEEpHHoCsc2R2qRRX/S6WeWl7J1PJvN+73IONRKHJYYitU7JoIgYDFPd+lcxYMUvbhW51TN6Fzc5LFtQVTYbZBUxuEEpHHoAcpca3bjAAAIABAACAAAAAgAIAAIAAAAAAAAAAACIGA2u7xjrIFe7ShVfXgPOqQwzKtxYkFeV52rNpNyzHfrNdHMR3/RMwAACAAQAAgAAAAIACAACAAAAAAAAAAAAAAA==
```
Output:
``` 
{
  "hex": "02000000000101312038558016444dd3db022b4575112f54c8c89441126b6f4006e23d72d2afbe0100000000fdffffff0118380f0000000000220020cb7df2661006338eb29c373eb4e8583cf29696ddfa3bdf35c347f18d7db4636202483045022100d61f7023a5c8fb7273eb57f7aeb47d2ffea0b876dc1bbfc39746b60a61215a520220573064aa8a59ef7a81490841cb659fa2bd47d5389c7c77a89c6b1d486692e7ff0141210314f77e95cc58314bdb856e754cde85cdce4b16d4154d86d9054c6e104a471e80ac736476a91457fd2e9679697b2753c9bcdfbbdc838d44a1c96188ad53b26800000000",
  "complete": true
}
```

And broadcast the resulting transaction:
```shell 
bitcoin-cli -signet sendrawtransaction 02000000000101312038558016444dd3db022b4575112f54c8c89441126b6f4006e23d72d2afbe0100000000fdffffff0118380f0000000000220020cb7df2661006338eb29c373eb4e8583cf29696ddfa3bdf35c347f18d7db4636202483045022100d61f7023a5c8fb7273eb57f7aeb47d2ffea0b876dc1bbfc39746b60a61215a520220573064aa8a59ef7a81490841cb659fa2bd47d5389c7c77a89c6b1d486692e7ff0141210314f77e95cc58314bdb856e754cde85cdce4b16d4154d86d9054c6e104a471e80ac736476a91457fd2e9679697b2753c9bcdfbbdc838d44a1c96188ad53b26800000000
```
