settings-language = Lingua
settings-language-description = Scegli la lingua usata dall'applicazione.
settings-fiat-price = Prezzo fiat:
settings-fiat-price-tooltip = I dati sui prezzi fiat sono forniti da servizi di terze parti. Disponibilità e accuratezza non sono garantite.
settings-exchange-rate-source = Fonte del tasso di cambio:
settings-currency = Valuta:
home-balance = Saldo
home-payment-history = Storico pagamenti
menu-dashboard = Dashboard
menu-receive = Ricevi
menu-drafts-approvals = Bozze e approvazioni
menu-transactions = Transazioni
menu-settings = Impostazioni
settings-section-general = Generale
settings-section-node = Nodo
settings-section-backend = Backend
settings-section-wallet = Wallet
settings-section-import-export = Importa/Esporta
settings-section-about = Informazioni
settings-import-wallet = Importa wallet
settings-import-wallet-description = Carica un file di backup per aggiornare le informazioni del wallet.
settings-export-wallet = Esporta wallet
settings-export-wallet-description = File (non criptato) con informazioni del wallet utile per sincronizzare etichette e dati su altri dispositivi.
settings-export-labels = Etichette BIP 329
settings-export-labels-description = Esportazione etichette BIP 329, compatibile con altri wallet.
settings-export-transactions = Tabella transazioni
settings-export-transactions-description = File .CSV delle transazioni passate, per fini contabili.
settings-export-descriptor = Solo descriptor - testo semplice
settings-export-descriptor-description = File descriptor in testo semplice (non criptato), da usare con altri wallet.
settings-export-encrypted-descriptor = Descriptor criptato
settings-export-encrypted-descriptor-description = File .bed, decifrabile con uno dei tuoi dispositivi di firma o xpub.
menu-coins-utxos = Monete/UTXO
menu-send = Invia
menu-recovery = Recupero
tab-close = Chiudi
tab-split = Dividi
tab-installer = Installazione
tab-loading = Caricamento...
tab-launcher = Avvio
tab-login = Accesso
common-select = Seleziona
common-login = Accedi
common-token = Token
common-go-back = Torna indietro
common-connection-failed = Connessione non riuscita
common-fetching = Recupero ...
common-see-more = Vedi altro
common-address-label = Indirizzo:
launcher-back-to-wallet-list = Torna alla lista wallet
launcher-share-xpubs = Condividi xpub
launcher-welcome-back = Bentornato
launcher-welcome = Benvenuto
launcher-add-wallet = Aggiungi wallet
launcher-create-new-wallet = Crea un nuovo wallet Liana
launcher-add-existing-wallet = Aggiungi un wallet Liana esistente
launcher-default-wallet-name = Il mio wallet Liana {$network}
launcher-delete-wallet = Elimina wallet
launcher-delete-local-config-question = Vuoi davvero eliminare la configurazione locale del wallet
launcher-delete-all-data-question = Vuoi davvero eliminare la configurazione e tutti i dati associati del wallet
launcher-delete-node-not-affected-this-network = (Il nodo Bitcoin gestito da Liana per questa rete non sarà interessato da questa azione.)
launcher-delete-node-not-affected = (Se usi un nodo Bitcoin gestito da Liana, non sarà interessato da questa azione.)
launcher-delete-warning-irreversible = ATTENZIONE: questa azione non può essere annullata.
launcher-delete-title-alias = Elimina configurazione per {$alias} (Liana-{$checksum})
launcher-delete-title = Elimina configurazione per Liana-{$checksum}
launcher-delete-connect-all-members = Elimina definitivamente questo wallet da Liana Connect (per tutti i membri).
launcher-delete-connect-disassociate = Dissocia anche {$email} da questo wallet Liana Connect.
launcher-wallet-deleted = Wallet eliminato correttamente
lianalite-token-expired = Il token è scaduto o non è valido
lianalite-wallet-deleted = Questo wallet è stato eliminato dal suo creatore per tutti i partecipanti e non può essere aperto. Per accedervi di nuovo, ripristinalo usando un file di backup o il descriptor del wallet.
lianalite-auth-sent = È stata inviata un'autenticazione alla tua email:
lianalite-token-invalid = Il token non è valido
lianalite-resend-token = Reinvia token
receive-verify-on-device = Verifica su hardware wallet
receive-show-qr = Mostra codice QR
receive-generate-address = Genera indirizzo
receive-generate-new-address-help = Genera sempre un nuovo indirizzo per ogni deposito.
receive-previous-addresses = Indirizzi generati in precedenza ancora in attesa di deposito
receive-derivation-index = Indice di derivazione:
receive-select-device = Seleziona il dispositivo su cui verificare l'indirizzo:
common-import = Importa
common-processing = Elaborazione...
common-new = Nuovo
common-export = Esporta
common-confirm = Conferma
common-next = Avanti
common-self-transfer = Autotrasferimento
common-from = Da
common-no-label = Nessuna etichetta
common-feerate = Feerate
psbts-insert-psbt = Inserisci PSBT:
psbts-base64-warning = Inserisci una PSBT codificata in base64
psbts-imported = PSBT importata
coins-recovery-available = Uno o più percorsi di recupero sono disponibili
coins-first-recovery-in-blocks = Il primo percorso di recupero sarà disponibile tra {$blocks} blocchi
coins-address-label = Etichetta indirizzo:
coins-deposit-transaction-label = Etichetta transazione di deposito:
coins-outpoint = Outpoint:
coins-block-height = Altezza blocco:
coins-spend-txid = Txid di spesa:
coins-spend-block-height = Altezza blocco di spesa:
coins-not-in-block = Non in un blocco
coins-refresh-coin = Aggiorna moneta
recovery-info = Recupera i tuoi fondi inviandoli a un altro wallet se hai perso l'accesso al percorso di spesa principale.
recovery-none-available = Nessun percorso di recupero è attualmente disponibile.
recovery-paths-available = { $count ->
    [one] 1 percorso di recupero disponibile:
   *[other] {$count} percorsi di recupero disponibili:
}
recovery-signatures-from = { $count ->
    [one] 1 firma da
   *[other] {$count} firme da
}
recovery-can-recover = può recuperare
recovery-coins-total = { $count ->
    [one] 1 moneta per un totale di
   *[other] {$count} monete per un totale di
}
transactions-rbf-cancel-help = Sostituisce la transazione con una che paga un feerate più alto e rimanda le monete al tuo wallet. Non c'è garanzia che la transazione originale non venga minata per prima. La transazione sostitutiva può usare nuovi input.
transactions-rbf-bump-help = Sostituisce la transazione con una che paga un feerate più alto per incentivare una conferma più rapida. La transazione sostitutiva può usare nuovi input.
transactions-replacement = Sostituzione transazione
transactions-rbf-invalidates-some = ATTENZIONE: sostituire questa transazione invaliderà alcuni pagamenti successivi.
transactions-rbf-invalidates-one = ATTENZIONE: sostituire questa transazione invaliderà un pagamento successivo.
transactions-rbf-descendants-some = Le seguenti transazioni spendono uno o più output della transazione da sostituire e saranno scartate quando la sostituzione verrà trasmessa, insieme a qualsiasi altra transazione che dipende da esse:
transactions-rbf-descendants-one = La seguente transazione spende uno o più output della transazione da sostituire e sarà scartata quando la sostituzione verrà trasmessa, insieme a qualsiasi altra transazione che dipende da essa:
transactions-rbf-feerate-warning = Il feerate deve essere maggiore del valore precedente e minore o uguale a 1000 sats/vbyte
transactions-rbf-created = PSBT sostitutiva creata correttamente e pronta per la firma
transactions-go-to-replacement = Vai alla sostituzione
transactions-transaction = Transazione
transactions-incoming = Transazione in entrata
transactions-outgoing = Transazione in uscita
transactions-miner-fee = Commissione miner:
transactions-bump-fee = Aumenta fee
transactions-cancel = Annulla transazione
transactions-cancel-tooltip = Tentativo best-effort di double spend di una transazione in uscita non confermata
transactions-date = Data:
transactions-txid = Txid:
common-delete = Elimina
common-previous = Indietro
common-save = Salva
common-clear = Cancella
common-address = Indirizzo
spend-batch-label = Etichetta batch
spend-label-too-long = Lunghezza etichetta non valida, non può essere superiore a 100
spend-duplicate-addresses = Due indirizzi di pagamento sono uguali
spend-add-payment = Aggiungi pagamento
spend-feerate-placeholder = 42 (in sats/vbyte)
spend-feerate-warning = Il feerate deve essere un intero minore o uguale a 1000 sats/vbyte
spend-fee = Fee:
spend-feerate = Feerate:
spend-selected = selezionato
spend-select-one-coin = Seleziona almeno una moneta.
spend-check-max-recipient = Controlla l'importo massimo per il destinatario.
spend-left-to-select = ancora da selezionare
spend-feerate-needed = Il feerate deve essere impostato.
spend-add-recipient-details = Aggiungi i dettagli del destinatario.
spend-select-or-add-funds = Seleziona o aggiungi altri fondi.
spend-coins-selection = Selezione monete
spend-invalid-address = Indirizzo non valido (forse è per un'altra rete?)
spend-description = Descrizione
spend-payment-label = Etichetta pagamento
spend-amount-btc = Importo (BTC)
spend-btc-placeholder = 0.001 (in BTC)
spend-invalid-amount = Importo non valido. (Gli importi inferiori a 0.000005 BTC non sono validi.)
spend-fiat-placeholder = Inserisci importo in {$currency}
spend-max-tooltip = Importo totale rimanente dopo fee ed eventuali altri destinatari
settings-import-export-description = Una raccolta delle funzioni di esportazione e importazione presenti in Liana.
settings-other-formats = Altri formati
settings-version = Versione
settings-grant-wallet-access = Concedi accesso al wallet a un altro utente
settings-user-email = Email utente
settings-email-invalid = Email non valida
settings-invitation-sent = Invito inviato
settings-send-invitation = Invia invito
settings-connect-own-node = Voglio connettermi al mio nodo
settings-network = Rete:
settings-block-height = Altezza blocco:
common-accept = Accetta
common-descriptor-label = Descriptor:
common-descriptor = Descriptor
common-or = oppure
common-something-wrong = Qualcosa è andato storto
installer-load-previous-wallet = Carica un wallet usato in precedenza
installer-no-current-wallets = Non hai wallet attuali
installer-load-shared-wallet = Carica un wallet condiviso
installer-shared-wallet-help = Se hai ricevuto un invito a unirti a un wallet condiviso
installer-invitation-token-help = Digita il token di invito ricevuto via email
installer-accept-invitation-for = Accetta invito per il wallet:
installer-paste-invitation = Incolla invito:
installer-invitation = Invito
installer-invitation-invalid = Il token di invito non è valido o è scaduto
installer-load-from-descriptor = Carica un wallet da descriptor
installer-load-from-descriptor-help = Crea un nuovo wallet dal descriptor
installer-descriptor-invalid = Il descriptor non è valido o non è compatibile con la rete
installer-import-descriptor = Importa descriptor
common-cancel = Annulla
common-overwrite = Sovrascrivi
common-ignore = Ignora
hw-descriptor-not-registered = Il descriptor del wallet non è registrato sul dispositivo.
 Puoi registrarlo nelle impostazioni.
hw-not-in-spending-path = Questo dispositivo di firma non fa parte di questo percorso di spesa.
hw-no-taproot-miniscript = La versione firmware del dispositivo non supporta taproot miniscript
hw-display-address-unavailable = Liana non può chiedere al dispositivo di mostrare l'indirizzo.
 La verifica deve essere effettuata manualmente con i controlli del dispositivo.
export-select-path = Seleziona il percorso da esportare nella finestra popup...
export-starting = Avvio esportazione...
export-progress = Avanzamento: {$progress}%
export-timeout = Esportazione non riuscita: timeout
export-canceled = Esportazione annullata
export-labels-conflict = Conflitto etichette, cosa vuoi fare?
export-aliases-conflict = Conflitto alias, cosa vuoi fare?
common-copy = Copia
common-learn-more = Scopri di più
installer-descriptor-wrong-network = Il descriptor è per un'altra rete
installer-descriptor-read-failed = Impossibile leggere il descriptor
installer-import-backup = Importa backup
installer-backup-imported = Backup importato correttamente!
installer-import-wallet-title = Importa il wallet
installer-import-wallet-rescan-help = Se usi un nodo Bitcoin Core, dovrai eseguire una rescansione della blockchain dopo aver creato il wallet per vedere le tue monete e le transazioni passate. Puoi farlo in Impostazioni > Nodo.
installer-invalid-descriptor = Descriptor non valido
installer-generate-mnemonic = Genera una nuova mnemonic
installer-backup-mnemonic-warning = Ricordati di fare il backup della mnemonic perché NON sarà salvata sul computer.
installer-switch-account-help = Cambia account se usi già lo stesso hardware in altre configurazioni
installer-import-xpub-device = Importa una chiave pubblica estesa selezionando un dispositivo di firma:
installer-share-xpubs-title = Condividi le tue chiavi pubbliche (xpub)
installer-no-device-connected = Nessun dispositivo di firma collegato
installer-create-random-key = Oppure crea una nuova chiave casuale:
installer-descriptor-template = Template descriptor
installer-the-descriptor = Il descriptor
installer-register-descriptor-optional = Questo passaggio è necessario solo se usi un dispositivo di firma.
installer-register-descriptor-failed = Registrazione descriptor non riuscita
installer-select-device-register = Seleziona hardware wallet su cui registrare il descriptor:
installer-select-device-register-if-needed = Se necessario, seleziona il dispositivo di firma su cui registrare il descriptor:
installer-registered-descriptor-checkbox = Ho registrato il descriptor sul/i mio/i dispositivo/i
installer-register-descriptor-title = Registra descriptor
installer-back-up-descriptor = Backup descriptor
installer-backup-descriptor-title = Fai il backup della configurazione del wallet (Descriptor)
installer-export-backup-failed = Esportazione backup non riuscita
installer-the-descriptor-label = Il descriptor:
installer-backed-up-descriptor-checkbox = Ho fatto il backup del mio descriptor
installer-node-type = Tipo nodo:
installer-checking-connection = Controllo connessione...
installer-connection-checked = Connessione verificata
installer-check-connection = Controlla connessione
installer-node-setup-title = Configura connessione al nodo Bitcoin
installer-enter-correct-address = Inserisci un indirizzo corretto
installer-remote-bitcoin-node-warning = La connessione a un nodo Bitcoin remoto non è supportata. Inserisci un indirizzo IP associato alla stessa macchina su cui è in esecuzione Liana (ignora questo avviso se è già così)
installer-rpc-auth = Autenticazione RPC:
installer-cookie-path = Percorso cookie
installer-enter-correct-path = Inserisci un percorso corretto
installer-user = Utente
installer-enter-correct-user = Inserisci un utente corretto
installer-password = Password
installer-enter-correct-password = Inserisci una password corretta
installer-enter-correct-electrum-address = Inserisci un indirizzo corretto (porta inclusa), opzionalmente preceduto da tcp:// o ssl://
settings-cookie-file-path = Percorso file cookie
settings-valid-filesystem-path = Inserisci un percorso filesystem valido
settings-valid-user = Inserisci un utente valido
settings-valid-password = Inserisci una password valida
settings-socket-address = Indirizzo socket:
settings-valid-address = Inserisci un indirizzo valido
settings-running = In esecuzione
settings-not-running = Non in esecuzione
settings-blockchain-rescan = Rescan blockchain
settings-rescan-success = Blockchain rescansionata correttamente
settings-rescanning = Rescan in corso...{$progress}%
settings-year = Anno:
settings-month = Mese:
settings-day = Giorno:
settings-date-invalid = La data fornita non è valida
settings-date-before-prune = La data fornita è precedente all'altezza di prune del nodo
settings-date-future = La data fornita è nel futuro
settings-start-rescan = Avvia rescan
settings-starting-rescan = Avvio rescan...
settings-backup-encrypted-descriptor = Backup descriptor cifrato
settings-backup-encrypted-descriptor-tooltip = Un file descriptor cifrato (.bed) che puoi conservare ovunque. Per decifrarlo, serve uno dei tuoi dispositivi di firma o xpub.
settings-wallet-descriptor = Descriptor wallet:
settings-register-on-device = Registra su hardware wallet
settings-wallet-alias = Alias wallet:
settings-alias = Alias
settings-alias-too-long = Inserisci un alias non troppo lungo
settings-fingerprint-aliases = Alias fingerprint:
settings-correct-alias = Inserisci un alias corretto
settings-updated = Aggiornato
settings-update = Aggiorna
settings-updating = Aggiornamento
common-and = e
common-blocks = blocchi
policy-signatures = { $count ->
    [one] 1 firma
   *[other] {$count} firme
}
policy-out-of-by = su {$count} da
policy-by = da
policy-primary-path = possono sempre spendere i fondi di questo wallet (percorso primario)
policy-inactive-for = possono spendere monete inattive per
policy-safety-net-path = (percorso Safety Net)
policy-recovery-path = (percorso di recupero n. {$number})
policy-wallet-policy = La policy del wallet:
settings-select-device = Seleziona dispositivo:
common-skip = Salta
common-email = Email
common-continue = Continua
installer-backed-up-mnemonic-show-xpub = Ho eseguito il backup della mnemonic, mostra la chiave pubblica estesa
installer-bitcoin-node-management = Gestione nodo Bitcoin
installer-already-have-node = Ho già un nodo
installer-auto-install-node = Voglio che Liana installi automaticamente un nodo Bitcoin sul mio dispositivo
installer-existing-node-description = Seleziona questa opzione se hai già un nodo Bitcoin in esecuzione localmente o da remoto. Liana si connetterà ad esso.
installer-managed-node-description = Liana installerà un nodo pruned sul tuo computer. Non dovrai fare nulla tranne avere spazio su disco disponibile (~30 GB richiesti su mainnet) e attendere la sincronizzazione iniziale con la rete (può richiedere alcuni giorni, a seconda della velocità della tua connessione internet).
installer-start-bitcoin-node = Avvia nodo completo Bitcoin
installer-download-complete = Download completato
installer-downloading-bitcoin-core = Download di Bitcoin Core {$version}
installer-download-failed = Download non riuscito: '{$error}'.
installer-installing-bitcoind = Installazione di bitcoind...
installer-installation-complete = Installazione completata
installer-installation-failed = Installazione non riuscita: '{$error}'.
installer-bitcoind-already-installed = bitcoind gestito da Liana già installato
installer-started = Avviato
installer-starting = Avvio...
installer-finalize-installation = Finalizza installazione
installer-installing = Installazione...
installer-installed = Installato
installer-threshold-keys = {$threshold} su {$total} chiavi
installer-available-after-inactivity = Disponibile dopo inattività di ~
installer-able-to-move-any-time = Può spostare i fondi in qualsiasi momento.
installer-backup-mnemonic-title = Backup della mnemonic
installer-backed-up-mnemonic-checkbox = Ho eseguito il backup della mia mnemonic
installer-import-mnemonic-title = Importa Mnemonic
installer-import-mnemonic = Importa mnemonic
installer-choose-backend = Scegli backend
installer-use-own-node = Usa il tuo nodo
installer-use-liana-connect = Usa Liana Connect
installer-local-wallet-description = Usa il tuo nodo Bitcoin esistente o installane uno automaticamente. Il wallet Liana non si connetterà ad alcun server esterno.

    Questa è l'opzione più privata, ma i dati sono conservati solo localmente su questo computer. Devi eseguire i tuoi backup e condividere il descriptor con le altre persone a cui vuoi consentire l'accesso al wallet.
installer-remote-backend-description = Usa il nostro servizio per essere subito pronto a transare. Wizardsardine gestisce l'infrastruttura, consentendo a più computer o partecipanti di connettersi e sincronizzarsi.

    È un'opzione più semplice e sicura per chi vuole che Wizardsardine conservi un backup del descriptor. Mantieni il controllo delle tue chiavi, e Wizardsardine non ha alcun controllo sui tuoi fondi, ma potrà vedere le informazioni del tuo wallet associate a un indirizzo email. Gli utenti attenti alla privacy dovrebbero gestire la propria infrastruttura.
installer-more-backend-node-info = Maggiori informazioni su backend e opzioni del nodo
installer-choose-existing-account = Scegli un account che stai già usando:
installer-enter-wallet-email = Inserisci un'email da associare al wallet:
installer-enter-new-wallet-email = Oppure inserisci una nuova email da associare al wallet:
installer-send-token = Invia token
installer-auth-token-emailed = Ti è stato inviato via email un token di autenticazione
installer-change-email = Cambia email
installer-give-wallet-alias = Dai un alias al tuo wallet
installer-wallet-alias = Alias wallet
installer-change-alias-later = Potrai modificarlo più tardi in Impostazioni > Wallet
common-edit = Modifica
common-set = Imposta
common-apply = Applica
common-replace = Sostituisci
common-retry = Riprova
installer-descriptor-type = Tipo di descriptor
installer-taproot-supported-version = Taproot è supportato solo da Liana versione 5.0 e successive
installer-add-safety-net-key = Aggiungi chiave Safety Net
installer-add-key = Aggiungi chiave
installer-keys-inactivity = Le chiavi possono spostare i fondi dopo inattività di:
installer-sequence-value-warning = Il valore deve essere superiore a 0 e inferiore a 65535
installer-threshold = Soglia:
installer-key-name-alias = Nome chiave (alias):
installer-key-name-help = Dai a questa chiave un nome descrittivo. Ti aiuterà a identificarla più tardi:
installer-key-alias-placeholder = Es. Il mio Hardware Wallet
installer-key-path-account = Account del percorso chiave:
installer-key-index = Chiave @{$index}:
decrypt-unlock-device = Sblocca o apri l'app sul dispositivo
decrypt-try-device = Prova a decifrare con questo dispositivo...
decrypt-device-failed = Impossibile decifrare il file con questo dispositivo
decrypt-device-description = Collega e sblocca un dispositivo hardware appartenente a questa configurazione per decifrare automaticamente il backup
decrypt-other-options = Altre opzioni
decrypt-airgap-help = Usi un dispositivo air-gapped? Esporta l'xpub dal dispositivo, poi usa l'opzione di caricamento o incolla. Se non conosci il percorso di derivazione corretto, prova con il seguente:
decrypt-provide-xpub = Fornisci uno degli xpub usati in questo wallet.
decrypt-upload-xpub-file = Carica file chiave pubblica estesa
decrypt-pairing-code = Codice di pairing: {$code}
decrypt-paste-xpub = Incolla una chiave pubblica estesa
decrypt-enter-mnemonic-unsafe = NON SICURO: Inserisci la mnemonic di una delle chiavi
decrypt-enter-mnemonic-warning = Questa opzione non è sicura. Capisco che inserire una mnemonic su un computer può causare il furto dei miei fondi.
decrypt-backup-file = Decifra file di backup
decrypt-invalid-encoding = Il file non può essere decodificato correttamente; sembra non essere un backup cifrato.
decrypt-invalid-type = Il file è stato decifrato ma il tipo di contenuto non è supportato.
decrypt-invalid-descriptor = Il file è stato decifrato ma il descriptor non è un descriptor Liana valido.
installer-introduction = Introduzione
installer-build-your-own = Crea il tuo setup
installer-custom-template-description-1 = Per questa configurazione dovrai definire le policy di spesa primaria e di recupero. Per motivi di sicurezza, suggeriamo di usare un Hardware Wallet separato per ogni chiave che vi appartiene.
installer-custom-template-description-2 = Le chiavi appartenenti alla policy primaria possono sempre spendere. Quelle delle policy di recupero potranno spendere solo dopo un tempo definito di inattività del wallet, consentendo recupero sicuro e policy di spesa avanzate.
installer-primary-spending-option = Opzione di spesa primaria:
installer-primary-key = Chiave primaria
installer-recovery-option = Opzione di recupero n. {$number}:
installer-recovery-key = Chiave di recupero
installer-add-recovery-option = Aggiungi opzione di recupero
installer-add-safety-net = Aggiungi Safety Net
installer-safety-net-description = Aggiunge un'opzione finale di recupero contenente chiavi di agenti professionali.

    Usa questa opzione se ti sono stati forniti uno o più token Safety Net.
installer-safety-net = Safety Net:
installer-safety-net-key = Chiave Safety Net
installer-set-keys = Imposta chiavi
installer-plug-hardware-device = Collega un dispositivo hardware ...
installer-detected-hardware = Hardware rilevato
installer-no-other-sources = - Nessun'altra fonte rilevata -
installer-already-used-sources = Fonti già usate
installer-advanced-settings = Impostazioni avanzate
common-clear-all = Cancella tutto
installer-customize = Personalizza
installer-choose-wallet-type = Scegli tipo di wallet
installer-simple-inheritance = Eredità semplice
installer-simple-inheritance-description = Sono richieste due chiavi, una per spendere e una per il tuo erede.
installer-expanding-multisig = Multisig espandibile
installer-expanding-multisig-description = Due chiavi richieste per spendere, con una chiave extra come backup.
installer-build-your-own-description = Crea una configurazione personalizzata adatta alle tue esigenze.
installer-simple-inheritance-wallet = Wallet con eredità semplice
installer-inheritance-description-1 = Per questa configurazione servono 2 chiavi: la tua chiave primaria (per te) e una chiave di eredità (per il tuo erede). Per motivi di sicurezza, suggeriamo di usare un Hardware Wallet separato per ogni chiave.
installer-inheritance-key = Chiave di eredità
installer-inheritance-description-2 = Potrai sempre spendere usando la tua chiave primaria. Dopo un periodo di inattività (ma non prima), la tua chiave di eredità potrà recuperare i fondi.
installer-device-no-taproot = Questo dispositivo non supporta Taproot
installer-expanding-multisig-wallet = Wallet multisig espandibile
installer-multisig-description-1 = Per questa configurazione servono 3 chiavi: due chiavi primarie e una chiave di recupero. Per motivi di sicurezza, suggeriamo di usare un Hardware Wallet separato per ogni chiave.
installer-primary-key-number = Chiave primaria n. {$number}
installer-multisig-description-2 = Le chiavi primarie comporranno una multisig 2-di-2 sempre in grado di spendere. Se una delle chiavi diventa non disponibile, dopo un periodo di inattività potrai recuperare i fondi usando la chiave di recupero insieme a una delle chiavi primarie (multisig 2-di-3):
installer-key-source-no-taproot = Questa fonte della chiave non supporta Taproot
common-update = Aggiorna
psbt-transaction-saved = Transazione salvata
psbt-save-transaction = Salva questa transazione
psbt-transaction-broadcast = Transazione trasmessa
psbt-broadcast-transaction = Trasmetti la transazione
psbt-broadcast = Trasmetti
psbt-broadcast-invalidates-some = ATTENZIONE: trasmettere questa transazione invaliderà alcuni pagamenti in sospeso.
psbt-broadcast-invalidates-one = ATTENZIONE: trasmettere questa transazione invaliderà un pagamento in sospeso.
psbt-broadcast-conflicts-some = Le seguenti transazioni stanno spendendo uno o più input della transazione da trasmettere e saranno scartate, insieme a qualsiasi altra transazione che dipende da esse:
psbt-broadcast-conflicts-one = La seguente transazione sta spendendo uno o più input della transazione da trasmettere e sarà scartata, insieme a qualsiasi altra transazione che dipende da essa:
psbt-delete-success = Transazione eliminata correttamente.
psbt-go-back = Torna ai PSBT
psbt-delete-this = Elimina questo PSBT
psbt-missing-inputs = Informazioni mancanti sugli input della transazione
psbt-sign-save-before-export = Firma o salva prima la transazione per abilitare l'esportazione
psbt-sign = Firma
psbt-status = Stato
psbt-ready = Pronto
psbt-signed-by = firmato da
psbt-not-ready = Non pronto
psbt-finalizing-requires = Per finalizzare questa transazione serve:
psbt-more-signatures = { $count ->
    [0] nessun'altra firma
    [one] 1 altra firma da
   *[other] {$count} altre firme da
}
psbt-already-signed-by = , già firmato da
psbt-coins-spent = { $count ->
    [one] 1 moneta spesa
   *[other] {$count} monete spese
}
psbt-payments = { $count ->
    [one] 1 pagamento
   *[other] {$count} pagamenti
}
psbt-no-payment = 0 pagamenti
psbt-change = Resto
psbt-select-signing-device = Seleziona dispositivo di firma:
psbt-device-sign-failed = Il dispositivo non ha firmato
psbt-label = PSBT:
psbt-insert-updated = Inserisci PSBT aggiornato:
psbt-base64-correct-warning = Inserisci il PSBT corretto codificato in base64
psbt-spend-updated = Transazione di spesa aggiornata
common-back = Indietro
payment-outgoing = Pagamento in uscita
payment-incoming = Pagamento in entrata
payment-title = Pagamento
payment-see-transaction-details = Vedi dettagli transazione
label-add = Aggiungi etichetta
label-label = Etichetta
label-invalid-length = Lunghezza etichetta non valida, non può essere maggiore di 100
loader-starting-daemon = Avvio daemon...
loader-connecting-daemon = Connessione al daemon...
loader-progress = Progresso {$progress}%
loader-sync-progress-1 = Bitcoin Core sta sincronizzando la blockchain. Una sincronizzazione completa richiede in genere alcuni giorni ed è intensiva in risorse. Una volta completata la sincronizzazione iniziale, le successive saranno molto più rapide.
loader-sync-progress-2 = Bitcoin Core sta sincronizzando la blockchain. Ci vorrà un po', a seconda dell'ultima volta in cui è stata eseguita, della connessione internet e delle prestazioni del computer.
loader-sync-progress-3 = Bitcoin Core sta sincronizzando la blockchain. Potrebbero volerci alcuni minuti, a seconda dell'ultima volta in cui è stata eseguita, della connessione internet e delle prestazioni del computer.
loader-failed-bitcoind = Liana non è riuscita ad avviarsi, verifica che bitcoind sia in esecuzione
loader-failed = Liana non è riuscita ad avviarsi
business-common-you = Tu
business-edited-by =  da {$name}
business-edited-relative = Modificato{$editor} {$time}
business-login-email-help = Inserisci l'email associata al tuo account
business-select-account = Seleziona un account per continuare
business-connect-another-email = Connettiti con un'altra email
installer-auth-token-emailed-to = Ti è stato inviato via email un token di autenticazione a
business-wallet-count = { $count ->
    [one] (1 wallet)
   *[other] ({$count} wallet)
}
business-key-count = { $count ->
    [one] (1 chiave)
   *[other] ({$count} chiavi)
}
business-contact-create-account = Contatta WizardSardine per creare un account.
business-no-orgs-search = Nessuna organizzazione trovata per la tua ricerca.
business-organizations = Organizzazioni
business-select-organization = Seleziona un'organizzazione
business-filter-organizations = Filtra organizzazioni...
business-organization = Organizzazione
business-wallet = Wallet
business-wallets = Wallet
business-select-wallet = Seleziona wallet
business-create-wallet = Crea un wallet
business-filter-wallets = Filtra wallet...
business-no-wallets-search = Nessun wallet trovato per la tua ricerca.
business-role-admin = Admin
business-role-manager = Manager
business-role-participant = Partecipante
business-keys = Chiavi
business-keys-instruction = Aggiungi le chiavi che faranno parte di questo wallet e collega ciascuna all'indirizzo email del proprietario.
business-add-key = + Aggiungi una chiave
business-unable-load-wallet = Impossibile caricare il wallet
business-service-unavailable = Il servizio è temporaneamente non disponibile. I dati del wallet e i fondi non sono stati interessati.
business-try-again-support = Riprova tra poco. Se il problema persiste, contatta il supporto.
business-loading-wallet = Caricamento wallet...
business-manage-keys = Gestisci chiavi
business-send-for-approval = Invia per approvazione
business-unlock = Sblocca
business-approve-template = Approva template
business-template = Template
business-set-keys = Imposta chiavi
business-your-key-set = La tua chiave è impostata.
business-your-keys-set = Le tue chiavi sono impostate.
business-wait-other-key-setup = Quando gli altri partecipanti completeranno la configurazione delle chiavi, potrai accedere al wallet.
business-xpub-instruction = Seleziona una chiave per completarne la configurazione. Le chiavi possono essere configurate individualmente da ogni key manager, oppure dal wallet manager per loro conto. Puoi collegare un dispositivo hardware (consigliato) o aggiungere manualmente una chiave pubblica estesa (xpub).
business-wallet-set-keys = {$wallet} - Imposta chiavi
business-no-keys-assigned = Nessuna chiave assegnata a te
business-no-keys-found = Nessuna chiave trovata
business-your-keys = Le tue chiavi:
business-other-participants-keys = Chiavi degli altri partecipanti:
business-register-devices = Registra dispositivi
business-register-wallet-devices = Registra wallet sui dispositivi
business-register-wallet-devices-help = Registra il descriptor del wallet su ogni dispositivo, oppure salta se non disponibile.
business-no-devices-register = Nessun dispositivo da registrare
business-no-devices-assigned = Non hai dispositivi assegnati in questo wallet.
business-register = Registra
business-device-unsupported-locked = Dispositivo non supportato o bloccato
business-connect-device-register = Collega il dispositivo associato per registrare
business-xpub-already-set-help = Questa chiave ha già un xpub. Puoi sostituirlo recuperandolo da un dispositivo, importandolo da file o incollandolo. Usa il pulsante Cancella per rimuoverlo completamente.
business-current-xpub = Xpub attuale:
business-select-key-source = Seleziona origine chiave - {$alias}
business-fetching-device = Recupero dal dispositivo...
business-account-number = Account n. {$index}
business-no-hardware-wallets = Nessun hardware wallet rilevato. Collega un dispositivo e sbloccalo.
business-detected-devices = Dispositivi rilevati:
business-unlock-device = Sblocca il dispositivo
business-not-part-wallet = Non fa parte di questo wallet (#{$fingerprint})
business-wrong-network-device = Rete errata nelle impostazioni del dispositivo
business-device-version-unsupported = Versione dispositivo non supportata, aggiorna a una versione > {$version}
business-unsupported-method = Metodo non supportato: {$method}
business-open-app-device = Apri l'app sul dispositivo
business-import-xpub-file = Importa file chiave pubblica estesa
business-edit-primary-path = Modifica percorso primario
business-edit-recovery-path = Modifica percorso di recupero
business-create-new-path = Crea nuovo percorso
business-keys-in-path = Chiavi nel percorso:
business-no-keys-available = Nessuna chiave disponibile. Aggiungi prima le chiavi.
business-key-number = Chiave {$id}
business-invalid-threshold = Valore soglia non valido
business-threshold-range = Soglia (1-{$count}):
business-timelock-zero = Il timelock non può essere zero
business-max-unit = Max {$max} {$unit}
business-duplicate-timelock = Timelock duplicato
business-timelock = Timelock:
business-max-unit-label = Max: {$max} {$unit}
business-no-timelock = Nessun timelock
business-after-months = { $count ->
    [one] Dopo 1 mese
   *[other] Dopo {$count} mesi
}
business-after-days = { $count ->
    [one] Dopo 1 giorno
   *[other] Dopo {$count} giorni
}
business-after-hours = { $count ->
    [one] Dopo 1 ora
   *[other] Dopo {$count} ore
}
business-no-keys = Nessuna chiave
business-all-of = Tutte di {$names}
business-threshold-of = {$threshold} di {$names}
business-spendable-anytime = Spendibile in qualsiasi momento
business-add-recovery-path = + Aggiungi percorso di recupero
business-confirm-device = Conferma sul dispositivo...
business-registering-wallet = Registrazione wallet
business-registration-failed = Registrazione non riuscita
business-confirm-coldcard-success = Conferma sulla tua Coldcard che la registrazione del wallet è stata completata correttamente.
business-did-registration-succeed = La registrazione sulla Coldcard è riuscita?
business-confirm-registration = Conferma registrazione
business-keep-my-changes = Mantieni le mie modifiche
common-reload = Ricarica
business-new-key = Nuova chiave
business-edit-key = Modifica chiave
business-key-alias = Alias chiave
business-enter-key-alias = Inserisci alias chiave
business-key-type = Tipo chiave
business-key-type-tooltip = Internal: chiavi detenute dalla tua organizzazione.
    External: chiavi detenute da terzi.
    Cosigner: chiave professionale di cofirma di terza parte.
    SafetyNet: chiave professionale di recupero di terza parte.
business-key-manager-email = Email del key manager
business-enter-email-address = Inserisci indirizzo email
business-enter-token-placeholder = Inserisci token (es. 42-absent-cake-eagle)
business-authenticated = Autenticato
business-connection-failed = Connessione non riuscita
business-user-session-not-found = Sessione utente non trovata. Accedi di nuovo o contatta WizardSardine.
business-access-error = Errore di accesso
business-wallet-access-denied = Non hai accesso a questo wallet. Contatta WizardSardine.
business-backend-error = Errore backend
business-connection-error = Errore di connessione
business-lost-connection-restart = Connessione al server persa. Riavvia l'applicazione.
business-account-connection-failed = Connessione con l'account {$email} non riuscita. La sessione potrebbe essere scaduta.
business-key-deleted = Chiave eliminata
business-key-deleted-message = La chiave che stavi modificando è stata eliminata da un altro utente.
business-key-modified = Chiave modificata
business-key-modified-message = Questa chiave è stata modificata da un altro utente. Vuoi ricaricare la versione del server o mantenere le modifiche?
business-key-removed = Chiave rimossa
business-key-removed-from-path = "{$alias}" è stata eliminata da un altro utente ed è stata rimossa dalla selezione del percorso.
business-path-modified = Percorso modificato
business-primary-path-modified-message = Il percorso primario è stato modificato da un altro utente. Vuoi ricaricare la versione del server o mantenere le modifiche?
business-path-deleted = Percorso eliminato
business-path-deleted-message = Il percorso che stavi modificando è stato eliminato da un altro utente.
business-recovery-path-modified-message = Questo percorso di recupero è stato modificato da un altro utente. Vuoi ricaricare la versione del server o mantenere le modifiche?
business-device-locked-unlock = Il dispositivo è bloccato. Sbloccalo prima.
business-device-not-supported = Dispositivo non supportato
business-hardware-wallet-not-found = Hardware wallet non trovato
business-select-xpub-file = Seleziona file xpub
business-text-files = File di testo
business-all-files = Tutti i file
business-file-read-failed = Impossibile leggere il file: {$error}
business-file-dialog-result-failed = Impossibile ricevere il risultato dalla finestra di dialogo file
business-clipboard-empty = Gli appunti sono vuoti
business-no-descriptor-available = Nessun descriptor disponibile
business-no-wallet-selected = Nessun wallet selezionato
business-no-user-id-available = Nessun ID utente disponibile
business-auth-code-request-failed = Impossibile richiedere il codice di autenticazione al server.
business-login-failed = Accesso non riuscito.
business-xpub-empty = La chiave pubblica estesa non può essere vuota.
business-xpub-invalid-format = Formato chiave pubblica estesa non valido: {$error}
business-xpub-invalid-network = La chiave pubblica estesa non è valida per {$network}.
business-device-disconnected = Dispositivo disconnesso
business-token-invalid = Token non valido.
business-token-duplicate = Token duplicato.
business-code-six-digits = Il codice deve contenere solo 6 cifre.
business-admin-name = Admin{$name}
time-just-now = adesso
time-minutes-ago = { $count ->
    [one] 1 minuto fa
   *[other] {$count} minuti fa
}
time-hours-ago = { $count ->
    [one] 1 ora fa
   *[other] {$count} ore fa
}
time-days-ago = { $count ->
    [one] 1 giorno fa
   *[other] {$count} giorni fa
}
time-weeks-ago = { $count ->
    [one] 1 settimana fa
   *[other] {$count} settimane fa
}
time-months-ago = { $count ->
    [one] 1 mese fa
   *[other] {$count} mesi fa
}
error-unknown = Errore sconosciuto
warning-wallet-error = Errore wallet
warning-fields-invalid = Alcuni campi non sono validi
warning-internal-error = Errore interno
warning-http-code-error = Errore HTTP {$code}: {$error}
warning-http-error = Errore HTTP: {$error}
warning-daemon-start-failed = Avvio daemon non riuscito
warning-daemon-client-unsupported = Client daemon non supportato
warning-daemon-communication-failed = Comunicazione con il daemon non riuscita
warning-daemon-stopped = Daemon arrestato
warning-coin-selection-error = Errore nella selezione delle coins da spendere
warning-backend-feature-unimplemented = Funzionalità non implementata per questo backend
warning-hardware-wallet-error = Errore hardware wallet
warning-descriptor-analysis-error = Errore di analisi descriptor: '{$error}'.
warning-spend-creation-error = Errore creazione spesa: '{$error}'.
warning-restore-backup-failed = Ripristino backup non riuscito: {$error}
warning-fiat-price-error = Errore prezzo fiat: {$error}
common-ok = OK
common-yes = Sì
common-no = No
common-reset-timelock = Reimposta timelock
common-go-to-rescan = Vai al rescan
common-dismiss = Ignora
pill-recovery = Recupero
pill-recovery-tooltip = Questa transazione usa un percorso di recupero
pill-batch = Batch
pill-batch-tooltip = Questa transazione contiene più pagamenti
pill-deprecated = Deprecata
pill-deprecated-tooltip = Questa transazione non può più essere inclusa nella blockchain.
pill-spent = Spesa
pill-spent-tooltip = La transazione è stata inclusa nella blockchain.
pill-unsigned = Non firmata
pill-unsigned-tooltip = A questa transazione mancano una o più firme
pill-signed = Da trasmettere
pill-signed-tooltip = Questa transazione è firmata e pronta per la trasmissione
pill-unconfirmed = Non confermata
pill-unconfirmed-tooltip = Non considerarla un pagamento finché non è confermata
pill-confirmed = Confermata
pill-confirmed-tooltip = Questa transazione è stata inclusa in un blocco
pill-key-internal = Internal
pill-key-internal-tooltip = Chiave detenuta dalla tua organizzazione
pill-key-external = External
pill-key-external-tooltip = Chiave detenuta da terzi
pill-key-cosigner = Cosigner
pill-key-cosigner-tooltip = Chiave professionale di cofirma di terza parte
pill-key-safety-net = Safety Net
pill-key-safety-net-tooltip = Chiave professionale di recupero di terza parte
pill-to-approve = Da approvare
pill-draft = Bozza
pill-set-keys = Imposta chiavi
pill-active = Attiva
pill-ws-admin = WS Admin
pill-register = Registra
pill-xpub-set = ✓ Impostata
pill-xpub-not-set = Non impostata
pill-rescan-progress = Rescan… {$progress}%
pill-available = Disponibile
pill-today = Oggi
pill-recovery-available-tooltip = Opzioni di recupero già disponibili
pill-first-recovery-today = Prima opzione di recupero disponibile oggi
pill-first-recovery-in = Prima opzione di recupero disponibile tra {$units}
duration-years = { $count ->
    [one] 1 anno
   *[other] {$count} anni
}
duration-months = { $count ->
    [one] 1 mese
   *[other] {$count} mesi
}
duration-days = { $count ->
    [one] 1 giorno
   *[other] {$count} giorni
}
duration-days-approx = ~{$count} giorni
duration-hours = { $count ->
    [one] 1 ora
   *[other] {$count} ore
}
duration-minutes = { $count ->
    [one] 1 minuto
   *[other] {$count} minuti
}
