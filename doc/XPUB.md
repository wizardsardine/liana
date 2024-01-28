# Get a participant XPub from another wallet:

In case one of the `cosigners` in your setup do not have possibility to use `Liana` to get its `xpub` he can also retrieve
from some other wallets/tools. You should pay attention to few points prior to import an XPub from outside of `Liana`:
 - If using an `Hardware Wallet` / `Signer`, the model should be supported by `Liana`, 
you can check [here](https://github.com/wizardsardine/async-hwi?tab=readme-ov-file#supported-devices).
 - You should register your newly created `Liana` descriptor on all `Hardware Devices` (some devices are limited 
in memory and in some case cannot register some _big_ descriptors), in order to validate there will not be any issue 
when you try to spend from your `primary` or `recovery` path prior to fund the wallet, if you can, try to test your setup 
on testnet/signet, signing with all `keys`/`devices`.

## Async-HWI

[`async-hwi`](https://github.com/wizardsardine/async-hwi/) is the underlying library used by `Liana` to connect to 
your `Hardware device`, so if you can get your XPub, you shouldn't have any issue with `Liana`. 
A [cli tool](https://github.com/wizardsardine/async-hwi/tree/master/cli) exposes the functions of the library, you can 
use it to retrieve an `xpub` from any supported `signing devices`.

### First get the fingerprint:
You can get the fingerprint by listing hardware devices:

``` 
hwi device list
```

you should get something like:
``` 
coldcard f25bdff6 6.2.1
```
`f25bdef6` is the `fingerprint` here

### Then get the xpub for `m/48'/0'/0'` derivation path

```
hwi xpub get --path "m/48'/0'/0'"
```
you should get something like:

``` 
xpub6E3wdqR3xPHvUKBWwUik5cpy9pMdrdEYVHBxKx7nbT2ZTnzizbNAWe9uuPX4A4nUsamM2Tn9F6ccK5Fmrt6ResBSRWDnb9J8bpi1WKcD158
```

the fingerprint you have to supply to `Liana` will be in the form:

``` 
[<fingerprint><derivation_path>]<xpub>
```

in our case:

``` 
[f25bdff6/48'/0'/0']xpub6E3wdqR3xPHvUKBWwUik5cpy9pMdrdEYVHBxKx7nbT2ZTnzizbNAWe9uuPX4A4nUsamM2Tn9F6ccK5Fmrt6ResBSRWDnb9J8bpi1WKcD158
```
## bitcoin core
Bitcoin core not yet have a simple command to export descriptor (see [#22341](https://github.com/bitcoin/bitcoin/pull/22341) 
and [#29130](https://github.com/bitcoin/bitcoin/pull/29130)) so you have to use `listdescriptors` command that will 
give you output descriptors that contains xpubs:

here we using `grep` command to keep only output containing descriptors and `rpcwallet=main` where 'main' is 
the wallet name (you can lists your wallets using `bitcoin-cli listwallets` command):
``` 
bitcoin-cli --rpcwallet=main listdescriptors | grep '"desc":'    
```

output should looks like this:
``` 
      "desc": "pkh([b6128dcb/44'/0'/0']xpub6CLAT4xwEBFHRfPqsBojCP2xCbv8kn3VoD7CG59D3Gt5a7Ba3yKsYjpusaWEmDqafb8Gg7ebv5Lg4vvrhMTiKcwJyH6kUk4J2GWARHHkvSo/0/*)#cd54nta7",
      "desc": "pkh([b6128dcb/44'/0'/0']xpub6CLAT4xwEBFHRfPqsBojCP2xCbv8kn3VoD7CG59D3Gt5a7Ba3yKsYjpusaWEmDqafb8Gg7ebv5Lg4vvrhMTiKcwJyH6kUk4J2GWARHHkvSo/1/*)#fe35w7dx",
      "desc": "sh(wpkh([b6128dcb/49'/0'/0']xpub6C75FRg1ZLnZ8uEfqWf9wyrZmaKPTqimaCnjMjQA3iKE8gLHC9HKttCcY5zHhAPc5uh8JE6JnJyVCQnVJyWvuVsPBhCFovKxUrYYkgGYik6/0/*))#70zrx66e",
      "desc": "sh(wpkh([b6128dcb/49'/0'/0']xpub6C75FRg1ZLnZ8uEfqWf9wyrZmaKPTqimaCnjMjQA3iKE8gLHC9HKttCcY5zHhAPc5uh8JE6JnJyVCQnVJyWvuVsPBhCFovKxUrYYkgGYik6/1/*))#twv4790x",
      "desc": "tr([b6128dcb/86'/0'/0']xpub6CV5jTV3fvR3h5Abv3FZabKUUm7S33cWg86dgUZ132aGZ9wGFCw6s65ACKcWXSjVt12oaWHLu6pFGp28HJtHteCkWxSyHhbjvfbyR7XooUn/0/*)#25anguum",
      "desc": "tr([b6128dcb/86'/0'/0']xpub6CV5jTV3fvR3h5Abv3FZabKUUm7S33cWg86dgUZ132aGZ9wGFCw6s65ACKcWXSjVt12oaWHLu6pFGp28HJtHteCkWxSyHhbjvfbyR7XooUn/1/*)#mqcj4fvr",
      "desc": "wpkh([b6128dcb/84'/0'/0']xpub6Bno9fbj6kSaQsrGzbSBLwuBPSQD8FErJHDAHuDarKQbPh5brv6pMHv68xbm9kxCNruPNYQ35QWiBgCEzoP3RbnRFrE2LgZAd9zDUTFTY6y/0/*)#03d4qhze",
      "desc": "wpkh([b6128dcb/84'/0'/0']xpub6Bno9fbj6kSaQsrGzbSBLwuBPSQD8FErJHDAHuDarKQbPh5brv6pMHv68xbm9kxCNruPNYQ35QWiBgCEzoP3RbnRFrE2LgZAd9zDUTFTY6y/1/*)#79g5azjp",
```

here you can use first taproot (`tr(...)`) or segwit (`wpkh(...)`) descriptor and extract the xpub, for instance if you
take first taproot descriptor:

`tr([b6128dcb/86'/0'/0']xpub6CV5jTV3fvR3h5Abv3FZabKUUm7S33cWg86dgUZ132aGZ9wGFCw6s65ACKcWXSjVt12oaWHLu6pFGp28HJtHteCkWxSyHhbjvfbyR7XooUn/0/*)#25anguum` 

the xpub you can load in `Liana` will be :

`[b6128dcb/86'/0'/0']xpub6CV5jTV3fvR3h5Abv3FZabKUUm7S33cWg86dgUZ132aGZ9wGFCw6s65ACKcWXSjVt12oaWHLu6pFGp28HJtHteCkWxSyHhbjvfbyR7XooUn`.



## Sparrow

![Sparrow.png](assets%2FXPUB%2FSparrow.png)

To get Xpub from your sparrow wallet you should:

* Go to `Settings` menu
* In the `Keystore` area, you should choose the tab of the key you want to export the `xpub` from and assembly the data
'by hand': 

``` 
[<master_fingerprint><derivation_path>]<xpub>
```

in our case:

``` 
[f25bdff6/84'/0'/0']xpub6BvHjS4FqVQLHB7KD86QJz8yfxtHYYCKLtyfptrUSKSEVRRgs21VVsBko8i9aCBVXmw4z24SkY8boG3KBEtA2uSADws2yQ66vQ8dj7rT9dJ
```

## Electrum

## Green

## Specter

## Wasabi

