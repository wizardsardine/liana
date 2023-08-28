Here a step by step walkthrough to recover a Liana wallet only using bitcoin-core and HWI utility.
Commands are given for signet, but you can run it in mainnet simply by not using the signet options.

We assume you already have a bitcoind/bitcoin-cli running on your machine.

### Create wallet in bitcoin core

```shell
bitcoin-cli -signet createwallet 'liana' true 
```

output:
```
{
  "name": "liana"
}
```

### Process & import descriptor

As PR[#22838](https://github.com/bitcoin/bitcoin/pull/22838) not (yet) merged in bitcoin core, we need to 'split'
descriptor in 2: 
 - 1 for receive addresses 
 - 1 for changes addresses.

#### Original descriptor:
 - wsh(or_d(pk([a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh/`<0;1>`/\*),and_v(v:pkh([c477fd13/48'/1'/0'/2']tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH/`<0;1>`/\*),older(3))))`#8ldsjayd`

Here we need to process 2 descriptors from the original one, replacing `<0;1>` by `0` for the receive descriptor and `1` for change descriptor, checksum might to be updated for each ones.

#### Processed descriptors:

 - wsh(or_d(pk([a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh/`0`/\*),and_v(v:pkh([c477fd13/48'/1'/0'/2']tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH/`0`/\*),older(3))))


 - wsh(or_d(pk([a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh/`1`/\*),and_v(v:pkh([c477fd13/48'/1'/0'/2']tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH/`1`/\*),older(3))))

Now we got the descriptors without checksums, we can process it using bitcoin-cli getdescriptorinfo method:

```shell
bitcoin-cli -signet getdescriptorinfo "wsh(or_d(pk([a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh/0/*),and_v(v:pkh([c477fd13/48'/1'/0'/2']tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH/0/*),older(3))))"
```

output:
``` 
{
  "descriptor": "wsh(or_d(pk([a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh/0/*),and_v(v:pkh([c477fd13/48'/1'/0'/2']tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH/0/*),older(3))))#nhtumqkr",
  "checksum": "nhtumqkr",
  "isrange": true,
  "issolvable": true,
  "hasprivatekeys": false
}
```

```shell
bitcoin-cli -signet getdescriptorinfo "wsh(or_d(pk([a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh/1/*),and_v(v:pkh([c477fd13/48'/1'/0'/2']tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH/1/*),older(3))))"
```

output:
``` 
{
  "descriptor": "wsh(or_d(pk([a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh/1/*),and_v(v:pkh([c477fd13/48'/1'/0'/2']tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH/1/*),older(3))))#vpa5k5p6",
  "checksum": "vpa5k5p6",
  "isrange": true,
  "issolvable": true,
  "hasprivatekeys": false
}
```

The command to pass to bitcoin-cli to import descriptors should look like this:

bitcoin-cli -signet -rpcwallet=liana importdescriptors "[{\"desc\": \"`<descriptor_receive>`\", \"range\": [0,10000], \"timestamp\": `<wallet_birthdate>`}, {\"desc\": \"`<descriptor_change>`\", \"range\": [0,10000], \"timestamp\": `<wallet_birthdate>`}]"

 - `<wallet_birthdate>` should be replaced by unix timestamp of your wallet birthdate(if you don't know about the wallet birthdate you can use  1671577200 -Liana V0.1 birthdate-, note that using a lower timestamp than the wallet birthdate will increase the rescan duration)
 - `<descriptor_receive>` and `<descriptor_change>` sould be replaced by relevant descriptors (including new generated checksums):

```shell
bitcoin-cli -signet -rpcwallet=liana importdescriptors "[{\"desc\": \"wsh(or_d(pk([a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh/0/*),and_v(v:pkh([c477fd13/48'/1'/0'/2']tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH/0/*),older(3))))#nhtumqkr\", \"range\": [0,10000], \"timestamp\": 1682920310, \"active\": true, \"internal\":false}, {\"desc\": \"wsh(or_d(pk([a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh/1/*),and_v(v:pkh([c477fd13/48'/1'/0'/2']tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH/1/*),older(3))))#vpa5k5p6\", \"range\": [0,10000], \"timestamp\": 1682920310, \"active\": true, \"internal\":true}]"
```

output:
```shell
[
  {
    "success": true
  },
  {
    "success": true
  }
]
```

### Build the sweep transaction

```shell 
bitcoin-cli -signet -rpcwallet=liana sendall '["tb1qed7lyessqcecav5uxultf6zc8nefd9kalgaa7dwrglcc6ld5vd3qe20spe"]' null "unset" 20
```
(here we define fee at `20 sats/VBytes`)

output:
```
{
  "psbt": "cHNidP8BAF4CAAAAATEgOFWAFkRN09sCK0V1ES9UyMiUQRJrb0AG4j1y0q++AQAAAAD9////ARg4DwAAAAAAIgAgy33yZhAGM46ynDc+tOhYPPKWlt36O981w0fxjX20Y2IAAAAAAAEAfQIAAAABnk7wzbybA7Z6kvlO7hFYl5x2BN3L0otR5M5udXzGN/oBAAAAAP7///8CrKE2EQAAAAAWABRnUU8zi4M3eMplKigSvOdhtGuSdUBCDwAAAAAAIgAgxBhOa53udP66VGgn+vs7sboB8aXyJj4WddV6PsUjJvHoZAIAAQErQEIPAAAAAAAiACDEGE5rne50/rpUaCf6+zuxugHxpfImPhZ11Xo+xSMm8QEFQSEDFPd+lcxYMUvbhW51TN6Fzc5LFtQVTYbZBUxuEEpHHoCsc2R2qRRX/S6WeWl7J1PJvN+73IONRKHJYYitU7JoIgYDFPd+lcxYMUvbhW51TN6Fzc5LFtQVTYbZBUxuEEpHHoAcpca3bjAAAIABAACAAAAAgAIAAIAAAAAAAAAAACIGA2u7xjrIFe7ShVfXgPOqQwzKtxYkFeV52rNpNyzHfrNdHMR3/RMwAACAAQAAgAAAAIACAACAAAAAAAAAAAAAAA==",
  "complete": false
}
```

### Install hwi
As [PR647(wallet policies)](https://github.com/bitcoin-core/HWI/pull/647) is not (yet) merged into HWI we need to install it from git:

#### Download the sources
```shell
git clone https://github.com/bitcoin-core/HWI
cd HWI
git fetch origin pull/647/head
git checkout FETCH_HEAD
```

#### Virtual environment & install
```shell
python3 -m venv venv
. venv/bin/activate
pip install poetry
poetry install
```

### Register policy on ledger nano:

Here we need to do (one more time) some processing on the descriptor format: the XPUBs might be extracted from descriptor and supplied as a list of keys (fingerprint + derivation path might be specified only for te key stored in the ledger nano)

```shell
python3 -m hwi --chain test --device-type ledger registerpolicy --policy "wsh(or_d(pk(@0/**),and_v(v:pkh(@1/**),older(3))))" --name "Liana" --keys "[\"[a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh\",\"tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH\"]"
```

output
``` 
{"proof_of_registration": "34602db12b76200ea96386c4cd10b8b1d60b84e060d2d6120073c06367a9506d"}
```

### Sign PSBT with ledger nano:

```shell
python3 -m hwi --chain test --device-type ledger signtx cHNidP8BAF4CAAAAATEgOFWAFkRN09sCK0V1ES9UyMiUQRJrb0AG4j1y0q++AQAAAAD9////ARg4DwAAAAAAIgAgy33yZhAGM46ynDc+tOhYPPKWlt36O981w0fxjX20Y2IAAAAAAAEAfQIAAAABnk7wzbybA7Z6kvlO7hFYl5x2BN3L0otR5M5udXzGN/oBAAAAAP7///8CrKE2EQAAAAAWABRnUU8zi4M3eMplKigSvOdhtGuSdUBCDwAAAAAAIgAgxBhOa53udP66VGgn+vs7sboB8aXyJj4WddV6PsUjJvHoZAIAAQErQEIPAAAAAAAiACDEGE5rne50/rpUaCf6+zuxugHxpfImPhZ11Xo+xSMm8QEFQSEDFPd+lcxYMUvbhW51TN6Fzc5LFtQVTYbZBUxuEEpHHoCsc2R2qRRX/S6WeWl7J1PJvN+73IONRKHJYYitU7JoIgYDFPd+lcxYMUvbhW51TN6Fzc5LFtQVTYbZBUxuEEpHHoAcpca3bjAAAIABAACAAAAAgAIAAIAAAAAAAAAAACIGA2u7xjrIFe7ShVfXgPOqQwzKtxYkFeV52rNpNyzHfrNdHMR3/RMwAACAAQAAgAAAAIACAACAAAAAAAAAAAAAAA== --policy "wsh(or_d(pk(@0/**),and_v(v:pkh(@1/**),older(3))))" --name "Liana" --keys "[\"[a5c6b76e/48'/1'/0'/2']tpubDF5861hj6vR3iJr3aPjGJz4rNbqDCRujQ21mczzKT5SiedaQqNVgHC8HT9ceyxvMFRoPMx4P6HAcL3NZrUPhRUbwCyj3TKSa64bAfnE3sLh\",\"tpubDFn7iPbFqGrTQ2aRACNsUK1MXQR4Z6dYfU2nD1WA9ifSaia642j3Wah4n5pBUEpERNWGJsyv3Dv5qwBabC9TLQrwSboKzukw9wmurGu7XVH\"]" --extra "{\"proof_of_registration\": \"34602db12b76200ea96386c4cd10b8b1d60b84e060d2d6120073c06367a9506d\"}"
```

output:
``` 
{"psbt": "cHNidP8BAF4CAAAAATEgOFWAFkRN09sCK0V1ES9UyMiUQRJrb0AG4j1y0q++AQAAAAD9////ARg4DwAAAAAAIgAgy33yZhAGM46ynDc+tOhYPPKWlt36O981w0fxjX20Y2IAAAAAAAEAfQIAAAABnk7wzbybA7Z6kvlO7hFYl5x2BN3L0otR5M5udXzGN/oBAAAAAP7///8CrKE2EQAAAAAWABRnUU8zi4M3eMplKigSvOdhtGuSdUBCDwAAAAAAIgAgxBhOa53udP66VGgn+vs7sboB8aXyJj4WddV6PsUjJvHoZAIAAQErQEIPAAAAAAAiACDEGE5rne50/rpUaCf6+zuxugHxpfImPhZ11Xo+xSMm8SICAxT3fpXMWDFL24VudUzehc3OSxbUFU2G2QVMbhBKRx6ASDBFAiEA1h9wI6XI+3Jz61f3rrR9L/6guHbcG7/Dl0a2CmEhWlICIFcwZKqKWe96gUkIQctln6K9R9U4nHx3qJxrHUhmkuf/AQEFQSEDFPd+lcxYMUvbhW51TN6Fzc5LFtQVTYbZBUxuEEpHHoCsc2R2qRRX/S6WeWl7J1PJvN+73IONRKHJYYitU7JoIgYDFPd+lcxYMUvbhW51TN6Fzc5LFtQVTYbZBUxuEEpHHoAcpca3bjAAAIABAACAAAAAgAIAAIAAAAAAAAAAACIGA2u7xjrIFe7ShVfXgPOqQwzKtxYkFeV52rNpNyzHfrNdHMR3/RMwAACAAQAAgAAAAIACAACAAAAAAAAAAAAAAA==", "signed": true}
```

### Finalize PSBT:

```shell
bitcoin-cli -signet finalizepsbt cHNidP8BAF4CAAAAATEgOFWAFkRN09sCK0V1ES9UyMiUQRJrb0AG4j1y0q++AQAAAAD9////ARg4DwAAAAAAIgAgy33yZhAGM46ynDc+tOhYPPKWlt36O981w0fxjX20Y2IAAAAAAAEAfQIAAAABnk7wzbybA7Z6kvlO7hFYl5x2BN3L0otR5M5udXzGN/oBAAAAAP7///8CrKE2EQAAAAAWABRnUU8zi4M3eMplKigSvOdhtGuSdUBCDwAAAAAAIgAgxBhOa53udP66VGgn+vs7sboB8aXyJj4WddV6PsUjJvHoZAIAAQErQEIPAAAAAAAiACDEGE5rne50/rpUaCf6+zuxugHxpfImPhZ11Xo+xSMm8SICAxT3fpXMWDFL24VudUzehc3OSxbUFU2G2QVMbhBKRx6ASDBFAiEA1h9wI6XI+3Jz61f3rrR9L/6guHbcG7/Dl0a2CmEhWlICIFcwZKqKWe96gUkIQctln6K9R9U4nHx3qJxrHUhmkuf/AQEFQSEDFPd+lcxYMUvbhW51TN6Fzc5LFtQVTYbZBUxuEEpHHoCsc2R2qRRX/S6WeWl7J1PJvN+73IONRKHJYYitU7JoIgYDFPd+lcxYMUvbhW51TN6Fzc5LFtQVTYbZBUxuEEpHHoAcpca3bjAAAIABAACAAAAAgAIAAIAAAAAAAAAAACIGA2u7xjrIFe7ShVfXgPOqQwzKtxYkFeV52rNpNyzHfrNdHMR3/RMwAACAAQAAgAAAAIACAACAAAAAAAAAAAAAAA==
```

output:
``` 
{
  "hex": "02000000000101312038558016444dd3db022b4575112f54c8c89441126b6f4006e23d72d2afbe0100000000fdffffff0118380f0000000000220020cb7df2661006338eb29c373eb4e8583cf29696ddfa3bdf35c347f18d7db4636202483045022100d61f7023a5c8fb7273eb57f7aeb47d2ffea0b876dc1bbfc39746b60a61215a520220573064aa8a59ef7a81490841cb659fa2bd47d5389c7c77a89c6b1d486692e7ff0141210314f77e95cc58314bdb856e754cde85cdce4b16d4154d86d9054c6e104a471e80ac736476a91457fd2e9679697b2753c9bcdfbbdc838d44a1c96188ad53b26800000000",
  "complete": true
}
```

### Broadcast PSBT:

```shell 
bitcoin-cli -signet sendrawtransaction 02000000000101312038558016444dd3db022b4575112f54c8c89441126b6f4006e23d72d2afbe0100000000fdffffff0118380f0000000000220020cb7df2661006338eb29c373eb4e8583cf29696ddfa3bdf35c347f18d7db4636202483045022100d61f7023a5c8fb7273eb57f7aeb47d2ffea0b876dc1bbfc39746b60a61215a520220573064aa8a59ef7a81490841cb659fa2bd47d5389c7c77a89c6b1d486692e7ff0141210314f77e95cc58314bdb856e754cde85cdce4b16d4154d86d9054c6e104a471e80ac736476a91457fd2e9679697b2753c9bcdfbbdc838d44a1c96188ad53b26800000000
```