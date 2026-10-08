# Labels

Every transaction, payment and coin in the wallet can have a label. If you have not
given an item a label, Liana shows a **default label** instead. The default comes
from an item that is related to it, such as the address that received a coin or the
coin that funded a transfer. A default label is shown with an info icon that says
where it came from. You can replace it with your own label at any time.

A label you write yourself always takes priority over a default.

## Coins and payments

A payment is one output of a transaction, as listed in the transaction details. Every
output paying you is also one of your coins, and the payment and the coin show the
same label. A payment to someone else is not a coin of yours.

The default label of an output is saved once, when the wallet first sees the
transaction that creates it. It is never computed again
(see [When defaults are saved](#when-defaults-are-saved)).

| The output is...                                       | It shows by default...       | Example                               |
|--------------------------------------------------------|------------------------------|---------------------------------------|
| a payment to someone else                              | the label of the transaction | tx "payroll": each shows "payroll"    |
| received from someone else, payjoin included           | the label of its address     | address "salary": coin shows "salary" |
| the change of a payment to **one** recipient           | the label of that payment    | you pay "rent": change shows "rent"   |
| the change of a payment to **several** recipients      | the label of the transaction | tx "payroll": change shows "payroll"  |
| the change of a payjoin you sent                       | the label of the transaction | tx "shop": change shows "shop"        |
| moved to yourself from **one** coin                    | the label shown on that coin | a "salary" coin moved: shows "salary" |
| consolidated from coins all showing the **same** label | that label                   | two "salary" coins: "salary"          |
| consolidated from coins whose labels **differ**        | nothing                      | "salary" + "rent": nothing            |

When you move coins to yourself, the label of the moving transaction is not used.
Only the labels of the coins being spent matter.

## Transactions

| The transaction...              | Shows by default...                     | Example                                  |
|---------------------------------|-----------------------------------------|------------------------------------------|
| receives one payment            | the label shown on the coin received    | address "salary": tx shows "salary"      |
| receives several payments       | nothing                                 |                                          |
| pays one recipient              | the label of that payment               | payment "rent": tx shows "rent"          |
| pays several recipients (batch) | nothing                                 |                                          |
| moves coins to yourself         | nothing, only its new coins inherit one | moving a "salary" coin: tx shows nothing |

## Moving coins several times

A coin moved to yourself takes the label shown on the coin it spends. That coin may
itself have come from an earlier move, so a label travels down the whole chain:

```
"salary" coin  ->  moved  ->  moved  ->  moved
  salary           salary     salary     salary
```

The chain changes in three cases:

1. **A coin in the chain gets its own label.** Coins created from it after that take
   the new label.

   ```
   "salary" coin  ->  moved, you label it "savings"  ->  moved
     salary             savings                          savings
   ```

2. **The chain goes through a consolidation.** The coin it creates follows the
   consolidation rows of the [Coins and payments](#coins-and-payments) table. For
   example if the labels of the coins it spends differ, the new coin has no label,
   and neither do the coins moved from it later.

   ```
   "salary" coin --+
                   +->  consolidated  ->  moved
   "rent" coin  ---+       (none)         (none)
   ```

3. **A label is changed after the move.** Coins already created keep the label they
   had. A coin created later still gets the old label too, because it reads the saved
   default of the coin it spends. The new text only travels down from a coin that has
   its own label.

## When defaults are saved

Default labels are saved by the daemon when it first sees a transaction, and never
computed again. Changing a label later does not rewrite the defaults of items that
already exist. The defaults are saved for the wallet's own coins and transactions
only.
