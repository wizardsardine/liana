# Signing devices

Documentation related to signing devices. For now only Specter and Ledger (at the latest version of
the application) are supported since we need them to support Miniscript descriptors.

The connection the signing devices is implemented in [another
repository](https://github.com/wizardsardine/async-hwi).


## Specter

[Specter DIY](https://github.com/cryptoadvance/specter-diy) version v1.5.0 and above is supported.

## Ledger

Only the latest version of the Bitcoin application (with full Miniscript descriptors support) is
supported. This is version 2.1.0.

Unfortunately the 2.1.0 Bitcoin application comes with a breaking change of the interface. Its
[rollout was paused](https://twitter.com/salvatoshi/status/1610663029913313280) due to compatibility
issues with existing applications. Since then the application is not available through the regular
app store channel anymore.

Its rollout was later [resumed **on testnet only**](https://twitter.com/salvatoshi/status/1612432385013956617).
In order to be able to install the latest Bitcoin testnet application from Ledger Live, first go to
settings (top-right corner on the home screen), then "Experimental features" and tick "Developer
mode". You will then be available to install the "Bitcoin test" application from the "My ledger"
panel.

In order to be able to install the latest Bitcoin application on mainnet, one more step is required.
Go to "settings" (top-right corner), then "Experimental features", enable "My Ledger provider" and
insert "4" in the field. You may then go to "My Ledger" and install the 2.1.0 Bitcoin application
for mainnet.

It's worth noting that although you need to tweak with the "Experimental features" settings, the
Bitcoin application is not experimental anymore. It was just downgraded in Ledger live from released
to experimental in order to prevent people from upgrading by mistake and not be able to keep using
applications not compatible with the new interface
