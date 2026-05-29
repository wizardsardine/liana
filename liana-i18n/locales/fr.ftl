settings-language = Langue
settings-language-description = Choisissez la langue utilisée par l'application.
settings-fiat-price = Prix en monnaie fiat :
settings-fiat-price-tooltip = Les données de prix en monnaie fiat sont fournies par des services tiers. Leur disponibilité et leur exactitude ne sont pas garanties.
settings-exchange-rate-source = Source du taux de change :
settings-currency = Devise :
home-balance = Solde
home-payment-history = Historique des paiements
menu-dashboard = Tableau de bord
menu-receive = Recevoir
menu-drafts-approvals = Brouillons et approbations
menu-transactions = Transactions
menu-settings = Paramètres
settings-section-general = Général
settings-section-node = Nœud
settings-section-backend = Backend
settings-section-wallet = Wallet
settings-section-import-export = Importer/Exporter
settings-section-about = À propos
settings-import-wallet = Importer un wallet
settings-import-wallet-description = Téléversez un fichier de backup pour mettre à jour les informations du wallet.
settings-export-wallet = Exporter le wallet
settings-export-wallet-description = Fichier (non chiffré) contenant des informations du wallet, utile pour synchroniser les libellés et données sur d'autres appareils.
settings-export-labels = Libellés BIP 329
settings-export-labels-description = Export de libellés BIP 329, compatible avec d'autres wallets.
settings-export-transactions = Tableau des transactions
settings-export-transactions-description = Fichier .CSV des transactions passées, à des fins comptables.
settings-export-descriptor = Descriptor seulement - texte brut
settings-export-descriptor-description = Fichier descriptor en texte brut (non chiffré), à utiliser avec d'autres wallets.
settings-export-encrypted-descriptor = Descriptor chiffré
settings-export-encrypted-descriptor-description = Fichier .bed, déchiffrable avec l'un de vos appareils de signature ou xpubs.
menu-coins-utxos = Pièces/UTXO
menu-send = Envoyer
menu-recovery = Récupération
tab-close = Fermer
tab-split = Diviser
tab-installer = Installation
tab-loading = Chargement...
tab-launcher = Lanceur
tab-login = Connexion
common-select = Sélectionner
common-login = Connexion
common-token = Token
common-go-back = Retour
common-connection-failed = Échec de la connexion
common-fetching = Récupération ...
common-see-more = Voir plus
common-address-label = Adresse :
launcher-back-to-wallet-list = Retour à la liste des wallets
launcher-share-xpubs = Partager les xpubs
launcher-welcome-back = Bon retour
launcher-welcome = Bienvenue
launcher-add-wallet = Ajouter un wallet
launcher-create-new-wallet = Créer un nouveau wallet Liana
launcher-add-existing-wallet = Ajouter un wallet Liana existant
launcher-default-wallet-name = Mon wallet Liana {$network}
launcher-delete-wallet = Supprimer le wallet
launcher-delete-local-config-question = Voulez-vous vraiment supprimer la configuration locale du wallet
launcher-delete-all-data-question = Voulez-vous vraiment supprimer la configuration et toutes les données associées du wallet
launcher-delete-node-not-affected-this-network = (Le nœud Bitcoin géré par Liana pour ce réseau ne sera pas affecté par cette action.)
launcher-delete-node-not-affected = (Si vous utilisez un nœud Bitcoin géré par Liana, il ne sera pas affecté par cette action.)
launcher-delete-warning-irreversible = AVERTISSEMENT : cette action est irréversible.
launcher-delete-title-alias = Supprimer la configuration de {$alias} (Liana-{$checksum})
launcher-delete-title = Supprimer la configuration de Liana-{$checksum}
launcher-delete-connect-all-members = Supprimer aussi définitivement ce wallet de Liana Connect (pour tous les membres).
launcher-delete-connect-disassociate = Dissocier aussi {$email} de ce wallet Liana Connect.
launcher-wallet-deleted = Wallet supprimé avec succès
lianalite-token-expired = Le token a expiré ou n'est pas valide
lianalite-wallet-deleted = Ce wallet a été supprimé par son créateur pour tous les participants et ne peut pas être ouvert. Pour y accéder à nouveau, restaurez-le avec un fichier de sauvegarde ou le descriptor du wallet.
lianalite-auth-sent = Une authentification a été envoyée à votre adresse e-mail :
lianalite-token-invalid = Le token n'est pas valide
lianalite-resend-token = Renvoyer le token
receive-verify-on-device = Vérifier sur hardware wallet
receive-show-qr = Afficher le QR code
receive-generate-address = Générer une adresse
receive-generate-new-address-help = Générez toujours une nouvelle adresse pour chaque dépôt.
receive-previous-addresses = Adresses générées précédemment toujours en attente de dépôt
receive-derivation-index = Index de dérivation :
receive-select-device = Sélectionnez le périphérique sur lequel vérifier l'adresse :
common-import = Importer
common-processing = Traitement...
common-new = Nouveau
common-export = Exporter
common-confirm = Confirmer
common-next = Suivant
common-self-transfer = Auto-transfert
common-from = Depuis
common-no-label = Aucun libellé
common-feerate = Feerate
psbts-insert-psbt = Insérer une PSBT :
psbts-base64-warning = Veuillez saisir une PSBT encodée en base64
psbts-imported = PSBT importée
coins-recovery-available = Un ou plusieurs chemins de récupération sont disponibles
coins-first-recovery-in-blocks = Le premier chemin de récupération sera disponible dans {$blocks} blocs
coins-address-label = Libellé de l'adresse :
coins-deposit-transaction-label = Libellé de la transaction de dépôt :
coins-outpoint = Outpoint :
coins-block-height = Hauteur de bloc :
coins-spend-txid = Txid de dépense :
coins-spend-block-height = Hauteur du bloc de dépense :
coins-not-in-block = Pas dans un bloc
coins-refresh-coin = Actualiser la pièce
recovery-info = Récupérez vos fonds en les envoyant vers un autre wallet si vous avez perdu l'accès à votre chemin de dépense principal.
recovery-none-available = Aucun chemin de récupération n'est actuellement disponible.
recovery-paths-available = { $count ->
    [one] 1 chemin de récupération est disponible :
   *[other] {$count} chemins de récupération sont disponibles :
}
recovery-signatures-from = { $count ->
    [one] 1 signature de
   *[other] {$count} signatures de
}
recovery-can-recover = peut récupérer
recovery-coins-total = { $count ->
    [one] 1 pièce totalisant
   *[other] {$count} pièces totalisant
}
transactions-rbf-cancel-help = Remplace la transaction par une autre avec un feerate plus élevé qui renvoie les pièces vers votre wallet. Rien ne garantit que la transaction originale ne sera pas minée en premier. De nouvelles entrées peuvent être utilisées pour la transaction de remplacement.
transactions-rbf-bump-help = Remplace la transaction par une autre avec un feerate plus élevé afin d'encourager une confirmation plus rapide. De nouvelles entrées peuvent être utilisées pour la transaction de remplacement.
transactions-replacement = Remplacement de transaction
transactions-rbf-invalidates-some = AVERTISSEMENT : remplacer cette transaction invalidera certains paiements ultérieurs.
transactions-rbf-invalidates-one = AVERTISSEMENT : remplacer cette transaction invalidera un paiement ultérieur.
transactions-rbf-descendants-some = Les transactions suivantes dépensent une ou plusieurs sorties de la transaction à remplacer et seront abandonnées lorsque le remplacement sera diffusé, ainsi que toute autre transaction qui en dépend :
transactions-rbf-descendants-one = La transaction suivante dépense une ou plusieurs sorties de la transaction à remplacer et sera abandonnée lorsque le remplacement sera diffusé, ainsi que toute autre transaction qui en dépend :
transactions-rbf-feerate-warning = Le feerate doit être supérieur à la valeur précédente et inférieur ou égal à 1000 sats/vbyte
transactions-rbf-created = PSBT de remplacement créée avec succès et prête à être signée
transactions-go-to-replacement = Aller au remplacement
transactions-transaction = Transaction
transactions-incoming = Transaction entrante
transactions-outgoing = Transaction sortante
transactions-miner-fee = Frais de minage :
transactions-bump-fee = Augmenter les frais
transactions-cancel = Annuler la transaction
transactions-cancel-tooltip = Tentative au mieux de double dépense d'une transaction sortante non confirmée
transactions-date = Date :
transactions-txid = Txid :
common-delete = Supprimer
common-previous = Précédent
common-save = Enregistrer
common-clear = Effacer
common-address = Adresse
spend-batch-label = Libellé du lot
spend-label-too-long = Longueur de libellé invalide, elle ne peut pas dépasser 100
spend-duplicate-addresses = Deux adresses de paiement sont identiques
spend-add-payment = Ajouter un paiement
spend-feerate-placeholder = 42 (en sats/vbyte)
spend-feerate-warning = Le feerate doit être un entier inférieur ou égal à 1000 sats/vbyte
spend-fee = Frais :
spend-feerate = Feerate :
spend-selected = sélectionné
spend-select-one-coin = Sélectionnez au moins une pièce.
spend-check-max-recipient = Vérifiez le montant maximum du destinataire.
spend-left-to-select = restant à sélectionner
spend-feerate-needed = Le feerate doit être défini.
spend-add-recipient-details = Ajoutez les détails du destinataire.
spend-select-or-add-funds = Sélectionnez ou ajoutez plus de fonds.
spend-coins-selection = Sélection des pièces
spend-invalid-address = Adresse invalide (peut-être pour un autre réseau ?)
spend-description = Description
spend-payment-label = Libellé du paiement
spend-amount-btc = Montant (BTC)
spend-btc-placeholder = 0.001 (en BTC)
spend-invalid-amount = Montant invalide. (Les montants inférieurs à 0,000005 BTC sont invalides.)
spend-fiat-placeholder = Saisir le montant en {$currency}
spend-max-tooltip = Montant total restant après paiement des frais et des autres destinataires
settings-import-export-description = Un ensemble des fonctions d'exportation et d'importation disponibles dans Liana.
settings-other-formats = Autres formats
settings-version = Version
settings-grant-wallet-access = Accorder l'accès au wallet à un autre utilisateur
settings-user-email = E-mail utilisateur
settings-email-invalid = L'e-mail est invalide
settings-invitation-sent = Invitation envoyée
settings-send-invitation = Envoyer l'invitation
settings-connect-own-node = Je veux me connecter à mon propre nœud
settings-network = Réseau :
settings-block-height = Hauteur de bloc :
common-accept = Accepter
common-descriptor-label = Descriptor :
common-descriptor = Descriptor
common-or = ou
common-something-wrong = Une erreur s'est produite
installer-load-previous-wallet = Charger un wallet utilisé précédemment
installer-no-current-wallets = Vous n'avez aucun wallet actuel
installer-load-shared-wallet = Charger un wallet partagé
installer-shared-wallet-help = Si vous avez reçu une invitation à rejoindre un wallet partagé
installer-invitation-token-help = Saisissez le token d'invitation reçu par e-mail
installer-accept-invitation-for = Accepter l'invitation pour le wallet :
installer-paste-invitation = Coller l'invitation :
installer-invitation = Invitation
installer-invitation-invalid = Le token d'invitation est invalide ou expiré
installer-load-from-descriptor = Charger un wallet depuis un descriptor
installer-load-from-descriptor-help = Crée un nouveau wallet à partir du descriptor
installer-descriptor-invalid = Le descriptor est invalide ou incompatible avec le réseau
installer-import-descriptor = Importer un descriptor
common-cancel = Annuler
common-overwrite = Écraser
common-ignore = Ignorer
hw-descriptor-not-registered = Le descriptor du wallet n'est pas enregistré sur le périphérique.
 Vous pouvez l'enregistrer dans les paramètres.
hw-not-in-spending-path = Ce périphérique de signature ne fait pas partie de ce chemin de dépense.
hw-no-taproot-miniscript = La version du firmware du périphérique ne prend pas en charge taproot miniscript
hw-display-address-unavailable = Liana ne peut pas demander au périphérique d'afficher l'adresse.
 La vérification doit être effectuée manuellement avec les contrôles du périphérique.
export-select-path = Sélectionnez le chemin à exporter dans la fenêtre contextuelle...
export-starting = Démarrage de l'exportation...
export-progress = Progression : {$progress} %
export-timeout = Échec de l'exportation : délai dépassé
export-canceled = Exportation annulée
export-labels-conflict = Conflit de libellés, que voulez-vous faire ?
export-aliases-conflict = Conflit d'alias, que voulez-vous faire ?
common-copy = Copier
common-learn-more = En savoir plus
installer-descriptor-wrong-network = Le descriptor correspond à un autre réseau
installer-descriptor-read-failed = Échec de lecture du descriptor
installer-import-backup = Importer une sauvegarde
installer-backup-imported = Sauvegarde importée avec succès !
installer-import-wallet-title = Importer le wallet
installer-import-wallet-rescan-help = Si vous utilisez un nœud Bitcoin Core, vous devrez rescanner la blockchain après la création du wallet afin de voir vos pièces et transactions passées. Cela peut être fait dans Paramètres > Nœud.
installer-invalid-descriptor = Descriptor invalide
installer-generate-mnemonic = Générer une nouvelle mnémonique
installer-backup-mnemonic-warning = Veillez à sauvegarder la mnémonique, car elle ne sera PAS stockée sur l'ordinateur.
installer-switch-account-help = Changez de compte si vous utilisez déjà le même matériel dans d'autres configurations
installer-import-xpub-device = Importez une clé publique étendue en sélectionnant un périphérique de signature :
installer-share-xpubs-title = Partager vos clés publiques (xpubs)
installer-no-device-connected = Aucun périphérique de signature connecté
installer-create-random-key = Ou créez une nouvelle clé aléatoire :
installer-descriptor-template = Modèle de descriptor
installer-the-descriptor = Le descriptor
installer-register-descriptor-optional = Cette étape n'est nécessaire que si vous utilisez un périphérique de signature.
installer-register-descriptor-failed = Échec de l'enregistrement du descriptor
installer-select-device-register = Sélectionnez le hardware wallet sur lequel enregistrer le descriptor :
installer-select-device-register-if-needed = Si nécessaire, sélectionnez le périphérique de signature sur lequel enregistrer le descriptor :
installer-registered-descriptor-checkbox = J'ai enregistré le descriptor sur mon/mes périphérique(s)
installer-register-descriptor-title = Enregistrer le descriptor
installer-back-up-descriptor = Sauvegarder le descriptor
installer-backup-descriptor-title = Sauvegardez la configuration de votre wallet (Descriptor)
installer-export-backup-failed = Échec de l'exportation de la sauvegarde
installer-the-descriptor-label = Le descriptor :
installer-backed-up-descriptor-checkbox = J'ai sauvegardé mon descriptor
installer-node-type = Type de nœud :
installer-checking-connection = Vérification de la connexion...
installer-connection-checked = Connexion vérifiée
installer-check-connection = Vérifier la connexion
installer-node-setup-title = Configurer la connexion au nœud Bitcoin
installer-enter-correct-address = Veuillez saisir une adresse correcte
installer-remote-bitcoin-node-warning = La connexion à un nœud Bitcoin distant n'est pas prise en charge. Saisissez une adresse IP liée à la même machine que celle exécutant Liana (ignorez cet avertissement si c'est déjà le cas)
installer-rpc-auth = Authentification RPC :
installer-cookie-path = Chemin du cookie
installer-enter-correct-path = Veuillez saisir un chemin correct
installer-user = Utilisateur
installer-enter-correct-user = Veuillez saisir un utilisateur correct
installer-password = Mot de passe
installer-enter-correct-password = Veuillez saisir un mot de passe correct
installer-enter-correct-electrum-address = Veuillez saisir une adresse correcte (port inclus), éventuellement préfixée par tcp:// ou ssl://
settings-cookie-file-path = Chemin du fichier cookie
settings-valid-filesystem-path = Veuillez saisir un chemin de fichier valide
settings-valid-user = Veuillez saisir un utilisateur valide
settings-valid-password = Veuillez saisir un mot de passe valide
settings-socket-address = Adresse du socket :
settings-valid-address = Veuillez saisir une adresse valide
settings-running = En cours d'exécution
settings-not-running = Non démarré
settings-blockchain-rescan = Rescan de la blockchain
settings-rescan-success = Blockchain rescannée avec succès
settings-rescanning = Rescan en cours...{$progress} %
settings-year = Année :
settings-month = Mois :
settings-day = Jour :
settings-date-invalid = La date fournie est invalide
settings-date-before-prune = La date fournie est antérieure à la hauteur de prune du nœud
settings-date-future = La date fournie est dans le futur
settings-start-rescan = Démarrer le rescan
settings-starting-rescan = Démarrage du rescan...
settings-backup-encrypted-descriptor = Sauvegarder le descriptor chiffré
settings-backup-encrypted-descriptor-tooltip = Un fichier descriptor chiffré (.bed) que vous pouvez stocker n'importe où. Pour le déchiffrer, vous avez besoin d'un de vos périphériques de signature ou xpubs.
settings-wallet-descriptor = Descriptor du wallet :
settings-register-on-device = Enregistrer sur hardware wallet
settings-wallet-alias = Alias du wallet :
settings-alias = Alias
settings-alias-too-long = Veuillez saisir un alias qui n'est pas trop long
settings-fingerprint-aliases = Alias des fingerprints :
settings-correct-alias = Veuillez saisir un alias correct
settings-updated = Mis à jour
settings-update = Mettre à jour
settings-updating = Mise à jour
common-and = et
common-blocks = blocs
policy-signatures = { $count ->
    [one] 1 signature
   *[other] {$count} signatures
}
policy-out-of-by = sur {$count} par
policy-by = par
policy-primary-path = peuvent toujours dépenser les fonds de ce wallet (chemin primaire)
policy-inactive-for = peuvent dépenser les pièces inactives pendant
policy-safety-net-path = (chemin Safety Net)
policy-recovery-path = (chemin de récupération n° {$number})
policy-wallet-policy = La politique du wallet :
settings-select-device = Sélectionner un appareil :
common-skip = Ignorer
common-email = E-mail
common-continue = Continuer
installer-backed-up-mnemonic-show-xpub = J'ai sauvegardé la mnemonic, afficher la clé publique étendue
installer-bitcoin-node-management = Gestion du nœud Bitcoin
installer-already-have-node = J'ai déjà un nœud
installer-auto-install-node = Je veux que Liana installe automatiquement un nœud Bitcoin sur mon appareil
installer-existing-node-description = Sélectionnez cette option si vous avez déjà un nœud Bitcoin en cours d'exécution localement ou à distance. Liana s'y connectera.
installer-managed-node-description = Liana installera un nœud pruned sur votre ordinateur. Vous n'aurez rien à faire, sauf disposer d'un peu d'espace disque disponible (~30 Go requis sur mainnet) et attendre la synchronisation initiale avec le réseau (cela peut prendre plusieurs jours selon la vitesse de votre connexion internet).
installer-start-bitcoin-node = Démarrer le nœud complet Bitcoin
installer-download-complete = Téléchargement terminé
installer-downloading-bitcoin-core = Téléchargement de Bitcoin Core {$version}
installer-download-failed = Échec du téléchargement : '{$error}'.
installer-installing-bitcoind = Installation de bitcoind...
installer-installation-complete = Installation terminée
installer-installation-failed = Échec de l'installation : '{$error}'.
installer-bitcoind-already-installed = bitcoind géré par Liana déjà installé
installer-started = Démarré
installer-starting = Démarrage...
installer-finalize-installation = Finaliser l'installation
installer-installing = Installation...
installer-installed = Installé
installer-threshold-keys = {$threshold} clés sur {$total}
installer-available-after-inactivity = Disponible après une inactivité de ~
installer-able-to-move-any-time = Peut déplacer les fonds à tout moment.
installer-backup-mnemonic-title = Sauvegarder votre mnemonic
installer-backed-up-mnemonic-checkbox = J'ai sauvegardé ma mnemonic
installer-import-mnemonic-title = Importer Mnemonic
installer-import-mnemonic = Importer la mnemonic
installer-choose-backend = Choisir le backend
installer-use-own-node = Utiliser votre propre nœud
installer-use-liana-connect = Utiliser Liana Connect
installer-local-wallet-description = Utilisez votre nœud Bitcoin existant ou installez-en un automatiquement. Le wallet Liana ne se connectera à aucun serveur externe.

    C'est l'option la plus privée, mais les données sont stockées uniquement localement sur cet ordinateur. Vous devez faire vos propres sauvegardes et partager le descriptor avec les autres personnes auxquelles vous voulez donner accès au wallet.
installer-remote-backend-description = Utilisez notre service pour être prêt à effectuer des transactions immédiatement. Wizardsardine exploite l'infrastructure, permettant à plusieurs ordinateurs ou participants de se connecter et de se synchroniser.

    C'est une option plus simple et plus sûre pour les personnes qui veulent que Wizardsardine conserve une sauvegarde de leur descriptor. Vous gardez le contrôle de vos clés, et Wizardsardine n'a aucun contrôle sur vos fonds, mais pourra voir les informations de votre wallet associées à une adresse e-mail. Les utilisateurs soucieux de leur vie privée devraient exploiter leur propre infrastructure.
installer-more-backend-node-info = Plus d'informations sur le backend et les options de nœud
installer-choose-existing-account = Choisissez un compte que vous utilisez déjà :
installer-enter-wallet-email = Saisissez une adresse e-mail à associer au wallet :
installer-enter-new-wallet-email = Ou saisissez une nouvelle adresse e-mail à associer au wallet :
installer-send-token = Envoyer le token
installer-auth-token-emailed = Un token d'authentification vous a été envoyé par e-mail
installer-change-email = Changer l'e-mail
installer-give-wallet-alias = Donnez un alias à votre wallet
installer-wallet-alias = Alias du wallet
installer-change-alias-later = Vous pourrez le modifier plus tard dans Paramètres > Wallet
common-edit = Modifier
common-set = Définir
common-apply = Appliquer
common-replace = Remplacer
common-retry = Réessayer
installer-descriptor-type = Type de descriptor
installer-taproot-supported-version = Taproot est uniquement pris en charge par Liana version 5.0 et supérieure
installer-add-safety-net-key = Ajouter une clé Safety Net
installer-add-key = Ajouter une clé
installer-keys-inactivity = Les clés peuvent déplacer les fonds après une inactivité de :
installer-sequence-value-warning = La valeur doit être supérieure à 0 et inférieure à 65535
installer-threshold = Seuil :
installer-key-name-alias = Nom de la clé (alias) :
installer-key-name-help = Donnez un nom convivial à cette clé. Cela vous aidera à l'identifier plus tard :
installer-key-alias-placeholder = Ex. Mon Hardware Wallet
installer-key-path-account = Compte du chemin de clé :
installer-key-index = Clé @{$index} :
decrypt-unlock-device = Veuillez déverrouiller ou ouvrir l'application sur l'appareil
decrypt-try-device = Essayer de déchiffrer avec cet appareil...
decrypt-device-failed = Échec du déchiffrement du fichier avec cet appareil
decrypt-device-description = Branchez et déverrouillez un appareil matériel appartenant à cette configuration pour déchiffrer automatiquement la sauvegarde
decrypt-other-options = Autres options
decrypt-airgap-help = Vous utilisez un appareil air-gapped ? Exportez le xpub depuis votre appareil, puis utilisez l'option de téléversement ou de collage. Si vous ne connaissez pas le bon chemin de dérivation, essayez avec le suivant :
decrypt-provide-xpub = Fournissez l'un des xpubs utilisés dans ce wallet.
decrypt-upload-xpub-file = Téléverser le fichier de clé publique étendue
decrypt-pairing-code = Code d'appairage : {$code}
decrypt-paste-xpub = Coller une clé publique étendue
decrypt-enter-mnemonic-unsafe = DANGEREUX : saisir la mnemonic d'une des clés
decrypt-enter-mnemonic-warning = Cette option n'est pas sûre. Je comprends que saisir une mnemonic sur un ordinateur peut entraîner le vol de mes fonds.
decrypt-backup-file = Déchiffrer le fichier de sauvegarde
decrypt-invalid-encoding = Le fichier ne peut pas être décodé correctement ; il ne semble pas être une sauvegarde chiffrée.
decrypt-invalid-type = Le fichier a été déchiffré mais le type de contenu n'est pas pris en charge.
decrypt-invalid-descriptor = Le fichier a été déchiffré mais le descriptor n'est pas un descriptor Liana valide.
installer-introduction = Introduction
installer-build-your-own = Construire votre propre configuration
installer-custom-template-description-1 = Pour cette configuration, vous devrez définir vos politiques de dépense primaire et de récupération. Pour des raisons de sécurité, nous vous suggérons d'utiliser un Hardware Wallet distinct pour chaque clé qui leur appartient.
installer-custom-template-description-2 = Les clés appartenant à votre politique primaire peuvent toujours dépenser. Celles des politiques de récupération ne pourront dépenser qu'après une durée définie d'inactivité du wallet, permettant une récupération sûre et des politiques de dépense avancées.
installer-primary-spending-option = Option de dépense primaire :
installer-primary-key = Clé primaire
installer-recovery-option = Option de récupération n° {$number} :
installer-recovery-key = Clé de récupération
installer-add-recovery-option = Ajouter une option de récupération
installer-add-safety-net = Ajouter Safety Net
installer-safety-net-description = Cela ajoute une option finale de récupération contenant des clés d'agents professionnels.

    Utilisez cette option si un ou plusieurs tokens Safety Net vous ont été fournis.
installer-safety-net = Safety Net :
installer-safety-net-key = Clé Safety Net
installer-set-keys = Définir les clés
installer-plug-hardware-device = Branchez un appareil matériel ...
installer-detected-hardware = Matériel détecté
installer-no-other-sources = - Aucune autre source détectée -
installer-already-used-sources = Sources déjà utilisées
installer-advanced-settings = Paramètres avancés
common-clear-all = Tout effacer
installer-customize = Personnaliser
installer-choose-wallet-type = Choisir le type de wallet
installer-simple-inheritance = Héritage simple
installer-simple-inheritance-description = Deux clés sont requises, une pour vous permettre de dépenser et une autre pour votre héritier.
installer-expanding-multisig = Multisig évolutif
installer-expanding-multisig-description = Deux clés sont requises pour dépenser, avec une clé supplémentaire en sauvegarde.
installer-build-your-own-description = Créez une configuration personnalisée adaptée à tous vos besoins.
installer-simple-inheritance-wallet = Wallet d'héritage simple
installer-inheritance-description-1 = Pour cette configuration, vous aurez besoin de 2 clés : votre clé primaire (pour vous) et une clé d'héritage (pour votre héritier). Pour des raisons de sécurité, nous vous suggérons d'utiliser un Hardware Wallet distinct pour chaque clé.
installer-inheritance-key = Clé d'héritage
installer-inheritance-description-2 = Vous pourrez toujours dépenser avec votre clé primaire. Après une période d'inactivité (mais pas avant), votre clé d'héritage pourra récupérer vos fonds.
installer-device-no-taproot = Cet appareil ne prend pas en charge Taproot
installer-expanding-multisig-wallet = Wallet multisig évolutif
installer-multisig-description-1 = Pour cette configuration, vous aurez besoin de 3 clés : deux clés primaires et une clé de récupération. Pour des raisons de sécurité, nous vous suggérons d'utiliser un Hardware Wallet distinct pour chaque clé.
installer-primary-key-number = Clé primaire n° {$number}
installer-multisig-description-2 = Les clés primaires composeront un multisig 2-sur-2 qui pourra toujours dépenser. Si une de vos clés devient indisponible, après une période d'inactivité vous pourrez récupérer vos fonds avec la clé de récupération et une de vos clés primaires (multisig 2-sur-3) :
installer-key-source-no-taproot = Cette source de clé ne prend pas en charge Taproot
common-update = Mettre à jour
psbt-transaction-saved = Transaction enregistrée
psbt-save-transaction = Enregistrer cette transaction
psbt-transaction-broadcast = Transaction diffusée
psbt-broadcast-transaction = Diffuser la transaction
psbt-broadcast = Diffuser
psbt-broadcast-invalidates-some = ATTENTION : diffuser cette transaction invalidera certains paiements en attente.
psbt-broadcast-invalidates-one = ATTENTION : diffuser cette transaction invalidera un paiement en attente.
psbt-broadcast-conflicts-some = Les transactions suivantes dépensent une ou plusieurs entrées de la transaction à diffuser et seront abandonnées, ainsi que toutes les autres transactions qui en dépendent :
psbt-broadcast-conflicts-one = La transaction suivante dépense une ou plusieurs entrées de la transaction à diffuser et sera abandonnée, ainsi que toutes les autres transactions qui en dépendent :
psbt-delete-success = Transaction supprimée avec succès.
psbt-go-back = Retour aux PSBTs
psbt-delete-this = Supprimer ce PSBT
psbt-missing-inputs = Informations manquantes sur les entrées de transaction
psbt-sign-save-before-export = Signez ou enregistrez d'abord la transaction pour activer l'export
psbt-sign = Signer
psbt-status = Statut
psbt-ready = Prêt
psbt-signed-by = signé par
psbt-not-ready = Pas prêt
psbt-finalizing-requires = La finalisation de cette transaction nécessite :
psbt-more-signatures = { $count ->
    [0] aucune signature supplémentaire
    [one] 1 signature supplémentaire de
   *[other] {$count} signatures supplémentaires de
}
psbt-already-signed-by = , déjà signé par
psbt-coins-spent = { $count ->
    [one] 1 pièce dépensée
   *[other] {$count} pièces dépensées
}
psbt-payments = { $count ->
    [one] 1 paiement
   *[other] {$count} paiements
}
psbt-no-payment = 0 paiement
psbt-change = Monnaie
psbt-select-signing-device = Sélectionner l'appareil de signature :
psbt-device-sign-failed = L'appareil n'a pas signé
psbt-label = PSBT :
psbt-insert-updated = Insérer le PSBT mis à jour :
psbt-base64-correct-warning = Veuillez saisir le bon PSBT encodé en base64
psbt-spend-updated = Transaction de dépense mise à jour
common-back = Retour
payment-outgoing = Paiement sortant
payment-incoming = Paiement entrant
payment-title = Paiement
payment-see-transaction-details = Voir les détails de la transaction
label-add = Ajouter une étiquette
label-label = Étiquette
label-invalid-length = Longueur d'étiquette invalide, elle ne peut pas dépasser 100
loader-starting-daemon = Démarrage du daemon...
loader-connecting-daemon = Connexion au daemon...
loader-progress = Progression {$progress} %
loader-sync-progress-1 = Bitcoin Core synchronise la blockchain. Une synchronisation complète prend généralement quelques jours et consomme beaucoup de ressources. Une fois la synchronisation initiale terminée, les suivantes seront beaucoup plus rapides.
loader-sync-progress-2 = Bitcoin Core synchronise la blockchain. Cela prendra un moment, selon la dernière fois où cela a été fait, votre connexion internet et les performances de votre ordinateur.
loader-sync-progress-3 = Bitcoin Core synchronise la blockchain. Cela peut prendre quelques minutes, selon la dernière fois où cela a été fait, votre connexion internet et les performances de votre ordinateur.
loader-failed-bitcoind = Liana n'a pas réussi à démarrer, veuillez vérifier que bitcoind est en cours d'exécution
loader-failed = Liana n'a pas réussi à démarrer
business-common-you = Vous
business-edited-by =  par {$name}
business-edited-relative = Modifié{$editor} {$time}
business-login-email-help = Saisissez l'e-mail associé à votre compte
business-select-account = Sélectionnez un compte pour continuer
business-connect-another-email = Se connecter avec une autre adresse e-mail
installer-auth-token-emailed-to = Un token d'authentification a été envoyé par e-mail à
business-wallet-count = { $count ->
    [one] (1 wallet)
   *[other] ({$count} wallets)
}
business-key-count = { $count ->
    [one] (1 clé)
   *[other] ({$count} clés)
}
business-contact-create-account = Contactez WizardSardine pour créer un compte.
business-no-orgs-search = Aucune organisation ne correspond à votre recherche.
business-organizations = Organisations
business-select-organization = Sélectionner une organisation
business-filter-organizations = Filtrer les organisations...
business-organization = Organisation
business-wallet = Wallet
business-wallets = Wallets
business-select-wallet = Sélectionner un wallet
business-create-wallet = Créer un wallet
business-filter-wallets = Filtrer les wallets...
business-no-wallets-search = Aucun wallet ne correspond à votre recherche.
business-role-admin = Admin
business-role-manager = Manager
business-role-participant = Participant
business-keys = Clés
business-keys-instruction = Ajoutez les clés qui feront partie de ce wallet et associez chacune à l'adresse e-mail de son propriétaire.
business-add-key = + Ajouter une clé
business-unable-load-wallet = Impossible de charger le wallet
business-service-unavailable = Le service est temporairement indisponible. Les données de votre wallet et vos fonds ne sont pas affectés.
business-try-again-support = Veuillez réessayer bientôt. Si le problème persiste, contactez le support.
business-loading-wallet = Chargement du wallet...
business-manage-keys = Gérer les clés
business-send-for-approval = Envoyer pour approbation
business-unlock = Déverrouiller
business-approve-template = Approuver le template
business-template = Template
business-set-keys = Définir les clés
business-your-key-set = Votre clé est définie.
business-your-keys-set = Vos clés sont définies.
business-wait-other-key-setup = Une fois que les autres participants auront terminé la configuration de leurs clés, vous pourrez accéder au wallet.
business-xpub-instruction = Sélectionnez une clé pour terminer sa configuration. Les clés peuvent être configurées individuellement par chaque gestionnaire de clé, ou par le gestionnaire du wallet en leur nom. Vous pouvez connecter un appareil matériel (recommandé) ou ajouter manuellement une clé publique étendue (xpub).
business-wallet-set-keys = {$wallet} - Définir les clés
business-no-keys-assigned = Aucune clé ne vous est assignée
business-no-keys-found = Aucune clé trouvée
business-your-keys = Vos clés :
business-other-participants-keys = Clés des autres participants :
business-register-devices = Enregistrer les appareils
business-register-wallet-devices = Enregistrer le wallet sur les appareils
business-register-wallet-devices-help = Enregistrez le descriptor du wallet sur chaque appareil, ou ignorez cette étape si indisponible.
business-no-devices-register = Aucun appareil à enregistrer
business-no-devices-assigned = Aucun appareil ne vous est assigné dans ce wallet.
business-register = Enregistrer
business-device-unsupported-locked = Appareil non pris en charge ou verrouillé
business-connect-device-register = Connectez l'appareil associé pour l'enregistrer
business-xpub-already-set-help = Cette clé a déjà un xpub. Vous pouvez le remplacer en le récupérant depuis un appareil, en l'important depuis un fichier ou en le collant. Utilisez le bouton Effacer pour le supprimer complètement.
business-current-xpub = Xpub actuel :
business-select-key-source = Sélectionner la source de clé - {$alias}
business-fetching-device = Récupération depuis l'appareil...
business-account-number = Compte n° {$index}
business-no-hardware-wallets = Aucun hardware wallet détecté. Connectez un appareil et déverrouillez-le.
business-detected-devices = Appareils détectés :
business-unlock-device = Veuillez déverrouiller l'appareil
business-not-part-wallet = Ne fait pas partie de ce wallet (#{$fingerprint})
business-wrong-network-device = Mauvais réseau dans les paramètres de l'appareil
business-device-version-unsupported = Version de l'appareil non prise en charge, mettez à jour vers une version > {$version}
business-unsupported-method = Méthode non prise en charge : {$method}
business-open-app-device = Veuillez ouvrir l'application sur l'appareil
business-import-xpub-file = Importer le fichier de clé publique étendue
business-edit-primary-path = Modifier le chemin primaire
business-edit-recovery-path = Modifier le chemin de récupération
business-create-new-path = Créer un nouveau chemin
business-keys-in-path = Clés dans le chemin :
business-no-keys-available = Aucune clé disponible. Ajoutez d'abord des clés.
business-key-number = Clé {$id}
business-invalid-threshold = Valeur de seuil invalide
business-threshold-range = Seuil (1-{$count}) :
business-timelock-zero = Le timelock ne peut pas être zéro
business-max-unit = Max {$max} {$unit}
business-duplicate-timelock = Timelock en double
business-timelock = Timelock :
business-max-unit-label = Max : {$max} {$unit}
business-no-timelock = Aucun timelock
business-after-months = { $count ->
    [one] Après 1 mois
   *[other] Après {$count} mois
}
business-after-days = { $count ->
    [one] Après 1 jour
   *[other] Après {$count} jours
}
business-after-hours = { $count ->
    [one] Après 1 heure
   *[other] Après {$count} heures
}
business-no-keys = Aucune clé
business-all-of = Toutes parmi {$names}
business-threshold-of = {$threshold} parmi {$names}
business-spendable-anytime = Dépensable à tout moment
business-add-recovery-path = + Ajouter un chemin de récupération
business-confirm-device = Veuillez confirmer sur votre appareil...
business-registering-wallet = Enregistrement du wallet
business-registration-failed = Échec de l'enregistrement
business-confirm-coldcard-success = Veuillez confirmer sur votre Coldcard que l'enregistrement du wallet s'est terminé avec succès.
business-did-registration-succeed = L'enregistrement a-t-il réussi sur votre Coldcard ?
business-confirm-registration = Confirmer l'enregistrement
business-keep-my-changes = Conserver mes modifications
common-reload = Recharger
business-new-key = Nouvelle clé
business-edit-key = Modifier la clé
business-key-alias = Alias de la clé
business-enter-key-alias = Saisir l'alias de la clé
business-key-type = Type de clé
business-key-type-tooltip = Internal : clés détenues par votre organisation.
    External : clés détenues par des tiers.
    Cosigner : clé professionnelle de cosignature tierce.
    SafetyNet : clé professionnelle de récupération tierce.
business-key-manager-email = E-mail du gestionnaire de clé
business-enter-email-address = Saisir l'adresse e-mail
business-enter-token-placeholder = Saisir le token (ex. 42-absent-cake-eagle)
business-authenticated = Authentifié
business-connection-failed = Échec de la connexion
business-user-session-not-found = Session utilisateur introuvable. Veuillez vous reconnecter ou contacter WizardSardine.
business-access-error = Erreur d'accès
business-wallet-access-denied = Vous n'avez pas accès à ce wallet. Contactez WizardSardine.
business-backend-error = Erreur du backend
business-connection-error = Erreur de connexion
business-lost-connection-restart = Connexion au serveur perdue. Veuillez redémarrer l'application.
business-account-connection-failed = Échec de la connexion avec le compte {$email}. La session a peut-être expiré.
business-key-deleted = Clé supprimée
business-key-deleted-message = La clé que vous modifiiez a été supprimée par un autre utilisateur.
business-key-modified = Clé modifiée
business-key-modified-message = Cette clé a été modifiée par un autre utilisateur. Voulez-vous recharger la version du serveur ou conserver vos modifications ?
business-key-removed = Clé retirée
business-key-removed-from-path = "{$alias}" a été supprimée par un autre utilisateur et retirée de votre sélection de chemin.
business-path-modified = Chemin modifié
business-primary-path-modified-message = Le chemin primaire a été modifié par un autre utilisateur. Voulez-vous recharger la version du serveur ou conserver vos modifications ?
business-path-deleted = Chemin supprimé
business-path-deleted-message = Le chemin que vous modifiiez a été supprimé par un autre utilisateur.
business-recovery-path-modified-message = Ce chemin de récupération a été modifié par un autre utilisateur. Voulez-vous recharger la version du serveur ou conserver vos modifications ?
business-device-locked-unlock = L'appareil est verrouillé. Veuillez d'abord le déverrouiller.
business-device-not-supported = Appareil non pris en charge
business-hardware-wallet-not-found = Hardware wallet introuvable
business-select-xpub-file = Sélectionner le fichier xpub
business-text-files = Fichiers texte
business-all-files = Tous les fichiers
business-file-read-failed = Échec de la lecture du fichier : {$error}
business-file-dialog-result-failed = Échec de la réception du résultat de la fenêtre de fichier
business-clipboard-empty = Le presse-papiers est vide
business-no-descriptor-available = Aucun descriptor disponible
business-no-wallet-selected = Aucun wallet sélectionné
business-no-user-id-available = Aucun ID utilisateur disponible
business-auth-code-request-failed = Échec de la demande de code d'authentification au serveur.
business-login-failed = Échec de la connexion.
business-xpub-empty = La clé publique étendue ne peut pas être vide.
business-xpub-invalid-format = Format de clé publique étendue invalide : {$error}
business-xpub-invalid-network = La clé publique étendue n'est pas valide pour {$network}.
business-device-disconnected = Appareil déconnecté
business-token-invalid = Token invalide.
business-token-duplicate = Token en double.
business-code-six-digits = Le code ne doit contenir que 6 chiffres.
business-admin-name = Admin{$name}
time-just-now = à l'instant
time-minutes-ago = { $count ->
    [one] il y a 1 minute
   *[other] il y a {$count} minutes
}
time-hours-ago = { $count ->
    [one] il y a 1 heure
   *[other] il y a {$count} heures
}
time-days-ago = { $count ->
    [one] il y a 1 jour
   *[other] il y a {$count} jours
}
time-weeks-ago = { $count ->
    [one] il y a 1 semaine
   *[other] il y a {$count} semaines
}
time-months-ago = { $count ->
    [one] il y a 1 mois
   *[other] il y a {$count} mois
}
error-unknown = Erreur inconnue
warning-wallet-error = Erreur du wallet
warning-fields-invalid = Certains champs sont invalides
warning-internal-error = Erreur interne
warning-http-code-error = Erreur HTTP {$code} : {$error}
warning-http-error = Erreur HTTP : {$error}
warning-daemon-start-failed = Échec du démarrage du daemon
warning-daemon-client-unsupported = Client du daemon non pris en charge
warning-daemon-communication-failed = Échec de la communication avec le daemon
warning-daemon-stopped = Daemon arrêté
warning-coin-selection-error = Erreur lors de la sélection des coins à dépenser
warning-backend-feature-unimplemented = Fonctionnalité non implémentée pour ce backend
warning-hardware-wallet-error = Erreur de hardware wallet
warning-descriptor-analysis-error = Erreur d'analyse du descriptor : '{$error}'.
warning-spend-creation-error = Erreur de création de dépense : '{$error}'.
warning-restore-backup-failed = Échec de la restauration du backup : {$error}
warning-fiat-price-error = Erreur de prix fiat : {$error}
common-ok = OK
common-yes = Oui
common-no = Non
common-reset-timelock = Réinitialiser le timelock
common-go-to-rescan = Aller au rescan
common-dismiss = Ignorer
pill-recovery = Récupération
pill-recovery-tooltip = Cette transaction utilise un chemin de récupération
pill-batch = Lot
pill-batch-tooltip = Cette transaction contient plusieurs paiements
pill-deprecated = Obsolète
pill-deprecated-tooltip = Cette transaction ne peut plus être incluse dans la blockchain.
pill-spent = Dépensée
pill-spent-tooltip = La transaction a été incluse dans la blockchain.
pill-unsigned = Non signée
pill-unsigned-tooltip = Il manque une ou plusieurs signatures à cette transaction
pill-signed = À diffuser
pill-signed-tooltip = Cette transaction est signée et prête à être diffusée
pill-unconfirmed = Non confirmée
pill-unconfirmed-tooltip = Ne considérez pas cela comme un paiement tant qu'il n'est pas confirmé
pill-confirmed = Confirmée
pill-confirmed-tooltip = Cette transaction a été incluse dans un bloc
pill-key-internal = Internal
pill-key-internal-tooltip = Clé détenue par votre organisation
pill-key-external = External
pill-key-external-tooltip = Clé détenue par des tiers
pill-key-cosigner = Cosigner
pill-key-cosigner-tooltip = Clé professionnelle de cosignature tierce
pill-key-safety-net = Safety Net
pill-key-safety-net-tooltip = Clé professionnelle de récupération tierce
pill-to-approve = À approuver
pill-draft = Brouillon
pill-set-keys = Définir les clés
pill-active = Active
pill-ws-admin = WS Admin
pill-register = Enregistrer
pill-xpub-set = ✓ Définie
pill-xpub-not-set = Non définie
pill-rescan-progress = Rescan… {$progress} %
pill-available = Disponible
pill-today = Aujourd'hui
pill-recovery-available-tooltip = Option(s) de récupération déjà disponibles
pill-first-recovery-today = Première option de récupération disponible aujourd'hui
pill-first-recovery-in = Première option de récupération disponible dans {$units}
duration-years = { $count ->
    [one] 1 an
   *[other] {$count} ans
}
duration-months = { $count ->
    [one] 1 mois
   *[other] {$count} mois
}
duration-days = { $count ->
    [one] 1 jour
   *[other] {$count} jours
}
duration-days-approx = ~{$count} jours
duration-hours = { $count ->
    [one] 1 heure
   *[other] {$count} heures
}
duration-minutes = { $count ->
    [one] 1 minute
   *[other] {$count} minutes
}
