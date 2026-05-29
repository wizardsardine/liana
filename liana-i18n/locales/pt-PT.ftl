settings-language = Idioma
settings-language-description = Escolha o idioma usado pela aplicação.
settings-fiat-price = Preço em moeda fiduciária:
settings-fiat-price-tooltip = Os dados de preço em moeda fiduciária são fornecidos por serviços de terceiros. A disponibilidade e a exactidão não são garantidas.
settings-exchange-rate-source = Fonte da taxa de câmbio:
settings-currency = Moeda:
home-balance = Saldo
home-payment-history = Histórico de pagamentos
menu-dashboard = Painel
menu-receive = Receber
menu-drafts-approvals = Rascunhos e aprovações
menu-transactions = Transações
menu-settings = Definições
settings-section-general = Geral
settings-section-node = Nó
settings-section-backend = Backend
settings-section-wallet = Carteira
settings-section-import-export = Importar/Exportar
settings-section-about = Sobre
settings-import-wallet = Importar carteira
settings-import-wallet-description = Carregue um ficheiro de backup para atualizar a informação da carteira.
settings-export-wallet = Exportar carteira
settings-export-wallet-description = Ficheiro (não encriptado) com informação da carteira, útil para sincronizar etiquetas e dados noutros dispositivos.
settings-export-labels = Etiquetas BIP 329
settings-export-labels-description = Exportação de etiquetas BIP 329, compatível com outras carteiras.
settings-export-transactions = Tabela de transações
settings-export-transactions-description = Ficheiro .CSV de transações passadas, para fins contabilísticos.
settings-export-descriptor = Apenas descriptor - texto simples
settings-export-descriptor-description = Ficheiro de descriptor em texto simples (não encriptado), para usar com outras carteiras.
settings-export-encrypted-descriptor = Descriptor encriptado
settings-export-encrypted-descriptor-description = Ficheiro .bed, que pode ser desencriptado com um dos seus dispositivos de assinatura ou xpubs.
menu-coins-utxos = Moedas/UTXOs
menu-send = Enviar
menu-recovery = Recuperação
tab-close = Fechar
tab-split = Dividir
tab-installer = Instalação
tab-loading = A carregar...
tab-launcher = Iniciador
tab-login = Iniciar sessão
common-select = Selecionar
common-login = Iniciar sessão
common-token = Token
common-go-back = Voltar
common-connection-failed = Falha na ligação
common-fetching = A obter ...
common-see-more = Ver mais
common-address-label = Endereço:
launcher-back-to-wallet-list = Voltar à lista de carteiras
launcher-share-xpubs = Partilhar xpubs
launcher-welcome-back = Bem-vindo de volta
launcher-welcome = Bem-vindo
launcher-add-wallet = Adicionar carteira
launcher-create-new-wallet = Criar uma nova carteira Liana
launcher-add-existing-wallet = Adicionar uma carteira Liana existente
launcher-default-wallet-name = A minha carteira Liana {$network}
launcher-delete-wallet = Apagar carteira
launcher-delete-local-config-question = Tem a certeza de que quer apagar a configuração local da carteira
launcher-delete-all-data-question = Tem a certeza de que quer apagar a configuração e todos os dados associados da carteira
launcher-delete-node-not-affected-this-network = (O nó Bitcoin gerido pela Liana para esta rede não será afetado por esta ação.)
launcher-delete-node-not-affected = (Se estiver a usar um nó Bitcoin gerido pela Liana, este não será afetado por esta ação.)
launcher-delete-warning-irreversible = AVISO: esta ação não pode ser anulada.
launcher-delete-title-alias = Apagar configuração de {$alias} (Liana-{$checksum})
launcher-delete-title = Apagar configuração de Liana-{$checksum}
launcher-delete-connect-all-members = Apagar também permanentemente esta carteira do Liana Connect (para todos os membros).
launcher-delete-connect-disassociate = Desassociar também {$email} desta carteira Liana Connect.
launcher-wallet-deleted = Carteira apagada com sucesso
lianalite-token-expired = O token expirou ou é inválido
lianalite-wallet-deleted = Esta carteira foi apagada pelo seu criador para todos os participantes e não pode ser aberta. Para voltar a aceder, restaure-a usando um ficheiro de cópia de segurança ou o descriptor da carteira.
lianalite-auth-sent = Foi enviada uma autenticação para o seu email:
lianalite-token-invalid = O token não é válido
lianalite-resend-token = Reenviar token
receive-verify-on-device = Verificar no hardware wallet
receive-show-qr = Mostrar código QR
receive-generate-address = Gerar endereço
receive-generate-new-address-help = Gere sempre um novo endereço para cada depósito.
receive-previous-addresses = Endereços gerados anteriormente ainda à espera de depósito
receive-derivation-index = Índice de derivação:
receive-select-device = Selecione o dispositivo para verificar o endereço:
common-import = Importar
common-processing = A processar...
common-new = Novo
common-export = Exportar
common-confirm = Confirmar
common-next = Seguinte
common-self-transfer = Auto-transferência
common-from = De
common-no-label = Sem etiqueta
common-feerate = Taxa
psbts-insert-psbt = Inserir PSBT:
psbts-base64-warning = Introduza uma PSBT codificada em base64
psbts-imported = PSBT importada
coins-recovery-available = Um ou mais caminhos de recuperação estão disponíveis
coins-first-recovery-in-blocks = O primeiro caminho de recuperação estará disponível em {$blocks} blocos
coins-address-label = Etiqueta do endereço:
coins-deposit-transaction-label = Etiqueta da transação de depósito:
coins-outpoint = Outpoint:
coins-block-height = Altura do bloco:
coins-spend-txid = Txid de gasto:
coins-spend-block-height = Altura do bloco de gasto:
coins-not-in-block = Não está num bloco
coins-refresh-coin = Atualizar moeda
recovery-info = Recupere os seus fundos enviando-os para outra carteira se tiver perdido o acesso ao caminho de gasto principal.
recovery-none-available = Nenhum caminho de recuperação está atualmente disponível.
recovery-paths-available = { $count ->
    [one] 1 caminho de recuperação está disponível:
   *[other] {$count} caminhos de recuperação estão disponíveis:
}
recovery-signatures-from = { $count ->
    [one] 1 assinatura de
   *[other] {$count} assinaturas de
}
recovery-can-recover = pode recuperar
recovery-coins-total = { $count ->
    [one] 1 moeda num total de
   *[other] {$count} moedas num total de
}
transactions-rbf-cancel-help = Substitui a transação por outra com uma taxa mais alta que envia as moedas de volta para a sua carteira. Não há garantia de que a transação original não seja minerada primeiro. Podem ser usadas novas entradas na transação de substituição.
transactions-rbf-bump-help = Substitui a transação por outra com uma taxa mais alta para incentivar uma confirmação mais rápida. Podem ser usadas novas entradas na transação de substituição.
transactions-replacement = Substituição de transação
transactions-rbf-invalidates-some = AVISO: substituir esta transação invalidará alguns pagamentos posteriores.
transactions-rbf-invalidates-one = AVISO: substituir esta transação invalidará um pagamento posterior.
transactions-rbf-descendants-some = As seguintes transações gastam uma ou mais saídas da transação a substituir e serão removidas quando a substituição for transmitida, juntamente com quaisquer outras transações que dependam delas:
transactions-rbf-descendants-one = A seguinte transação gasta uma ou mais saídas da transação a substituir e será removida quando a substituição for transmitida, juntamente com quaisquer outras transações que dependam dela:
transactions-rbf-feerate-warning = A taxa deve ser superior ao valor anterior e menor ou igual a 1000 sats/vbyte
transactions-rbf-created = PSBT de substituição criada com sucesso e pronta para ser assinada
transactions-go-to-replacement = Ir para a substituição
transactions-transaction = Transação
transactions-incoming = Transação recebida
transactions-outgoing = Transação enviada
transactions-miner-fee = Taxa de mineração:
transactions-bump-fee = Aumentar taxa
transactions-cancel = Cancelar transação
transactions-cancel-tooltip = Tentativa de melhor esforço de duplo gasto de uma transação enviada ainda não confirmada
transactions-date = Data:
transactions-txid = Txid:
common-delete = Apagar
common-previous = Anterior
common-save = Guardar
common-clear = Limpar
common-address = Endereço
spend-batch-label = Etiqueta do lote
spend-label-too-long = Comprimento de etiqueta inválido; não pode ser superior a 100
spend-duplicate-addresses = Dois endereços de pagamento são iguais
spend-add-payment = Adicionar pagamento
spend-feerate-placeholder = 42 (em sats/vbyte)
spend-feerate-warning = A taxa deve ser um número inteiro menor ou igual a 1000 sats/vbyte
spend-fee = Taxa:
spend-feerate = Taxa:
spend-selected = selecionado
spend-select-one-coin = Selecione pelo menos uma moeda.
spend-check-max-recipient = Verifique o montante máximo do destinatário.
spend-left-to-select = por selecionar
spend-feerate-needed = A taxa tem de ser definida.
spend-add-recipient-details = Adicione os dados do destinatário.
spend-select-or-add-funds = Selecione ou adicione mais fundos.
spend-coins-selection = Seleção de moedas
spend-invalid-address = Endereço inválido (talvez seja de outra rede?)
spend-description = Descrição
spend-payment-label = Etiqueta do pagamento
spend-amount-btc = Montante (BTC)
spend-btc-placeholder = 0.001 (em BTC)
spend-invalid-amount = Montante inválido. (Valores inferiores a 0,000005 BTC são inválidos.)
spend-fiat-placeholder = Introduza o montante em {$currency}
spend-max-tooltip = Montante total restante depois de pagar a taxa e quaisquer outros destinatários
settings-import-export-description = Um conjunto das funções de exportação e importação presentes na Liana.
settings-other-formats = Outros formatos
settings-version = Versão
settings-grant-wallet-access = Conceder acesso à carteira a outro utilizador
settings-user-email = Email do utilizador
settings-email-invalid = O email é inválido
settings-invitation-sent = Convite enviado
settings-send-invitation = Enviar convite
settings-connect-own-node = Quero ligar-me ao meu próprio nó
settings-network = Rede:
settings-block-height = Altura do bloco:
common-accept = Aceitar
common-descriptor-label = Descriptor:
common-descriptor = Descriptor
common-or = ou
common-something-wrong = Ocorreu um erro
installer-load-previous-wallet = Carregar uma carteira usada anteriormente
installer-no-current-wallets = Não tem carteiras atuais
installer-load-shared-wallet = Carregar uma carteira partilhada
installer-shared-wallet-help = Se recebeu um convite para aderir a uma carteira partilhada
installer-invitation-token-help = Introduza o token de convite que recebeu por email
installer-accept-invitation-for = Aceitar convite para a carteira:
installer-paste-invitation = Colar convite:
installer-invitation = Convite
installer-invitation-invalid = O token de convite é inválido ou expirou
installer-load-from-descriptor = Carregar uma carteira a partir do descriptor
installer-load-from-descriptor-help = Cria uma nova carteira a partir do descriptor
installer-descriptor-invalid = O descriptor é inválido ou incompatível com a rede
installer-import-descriptor = Importar descriptor
common-cancel = Cancelar
common-overwrite = Substituir
common-ignore = Ignorar
hw-descriptor-not-registered = O descriptor da carteira não está registado no dispositivo.
 Pode registá-lo nas definições.
hw-not-in-spending-path = Este dispositivo de assinatura não faz parte deste caminho de gasto.
hw-no-taproot-miniscript = A versão do firmware do dispositivo não suporta miniscript taproot
hw-display-address-unavailable = A Liana não consegue pedir ao dispositivo para mostrar o endereço.
 A verificação deve ser feita manualmente com os controlos do dispositivo.
export-select-path = Selecione o caminho que quer exportar na janela de popup...
export-starting = A iniciar exportação...
export-progress = Progresso: {$progress}%
export-timeout = Falha na exportação: tempo esgotado
export-canceled = Exportação cancelada
export-labels-conflict = Conflito de etiquetas, o que quer fazer?
export-aliases-conflict = Conflito de aliases, o que quer fazer?
common-copy = Copiar
common-learn-more = Saber mais
installer-descriptor-wrong-network = O descriptor é de outra rede
installer-descriptor-read-failed = Falha ao ler o descriptor
installer-import-backup = Importar cópia de segurança
installer-backup-imported = Cópia de segurança importada com sucesso!
installer-import-wallet-title = Importar a carteira
installer-import-wallet-rescan-help = Se estiver a usar um nó Bitcoin Core, terá de fazer uma nova análise da blockchain depois de criar a carteira para ver as suas moedas e transações passadas. Isto pode ser feito em Definições > Nó.
installer-invalid-descriptor = Descriptor inválido
installer-generate-mnemonic = Gerar uma nova mnemónica
installer-backup-mnemonic-warning = Atenção: faça uma cópia de segurança da mnemónica, pois ela NÃO será guardada no computador.
installer-switch-account-help = Mude de conta se já usa o mesmo hardware noutras configurações
installer-import-xpub-device = Importe uma chave pública alargada selecionando um dispositivo de assinatura:
installer-share-xpubs-title = Partilhe as suas chaves públicas (xpubs)
installer-no-device-connected = Nenhum dispositivo de assinatura ligado
installer-create-random-key = Ou crie uma nova chave aleatória:
installer-descriptor-template = Modelo de descriptor
installer-the-descriptor = O descriptor
installer-register-descriptor-optional = Este passo só é necessário se estiver a usar um dispositivo de assinatura.
installer-register-descriptor-failed = Falha ao registar o descriptor
installer-select-device-register = Selecione o hardware wallet onde registar o descriptor:
installer-select-device-register-if-needed = Se necessário, selecione o dispositivo de assinatura onde registar o descriptor:
installer-registered-descriptor-checkbox = Registei o descriptor no(s) meu(s) dispositivo(s)
installer-register-descriptor-title = Registar descriptor
installer-back-up-descriptor = Fazer cópia de segurança do descriptor
installer-backup-descriptor-title = Faça cópia de segurança da configuração da sua carteira (Descriptor)
installer-export-backup-failed = Falha ao exportar a cópia de segurança
installer-the-descriptor-label = O descriptor:
installer-backed-up-descriptor-checkbox = Fiz cópia de segurança do meu descriptor
installer-node-type = Tipo de nó:
installer-checking-connection = A verificar ligação...
installer-connection-checked = Ligação verificada
installer-check-connection = Verificar ligação
installer-node-setup-title = Configurar ligação ao nó Bitcoin
installer-enter-correct-address = Introduza um endereço correto
installer-remote-bitcoin-node-warning = A ligação a um nó Bitcoin remoto não é suportada. Introduza um endereço IP associado à mesma máquina onde a Liana está a correr (ignore este aviso se já for o caso)
installer-rpc-auth = Autenticação RPC:
installer-cookie-path = Caminho do cookie
installer-enter-correct-path = Introduza um caminho correto
installer-user = Utilizador
installer-enter-correct-user = Introduza um utilizador correto
installer-password = Palavra-passe
installer-enter-correct-password = Introduza uma palavra-passe correta
installer-enter-correct-electrum-address = Introduza um endereço correto (incluindo porta), opcionalmente prefixado com tcp:// ou ssl://
settings-cookie-file-path = Caminho do ficheiro cookie
settings-valid-filesystem-path = Introduza um caminho de ficheiro válido
settings-valid-user = Introduza um utilizador válido
settings-valid-password = Introduza uma palavra-passe válida
settings-socket-address = Endereço do socket:
settings-valid-address = Introduza um endereço válido
settings-running = Em execução
settings-not-running = Não está em execução
settings-blockchain-rescan = Nova análise da blockchain
settings-rescan-success = Blockchain analisada novamente com sucesso
settings-rescanning = A analisar novamente...{$progress}%
settings-year = Ano:
settings-month = Mês:
settings-day = Dia:
settings-date-invalid = A data indicada é inválida
settings-date-before-prune = A data indicada é anterior à altura de prune do nó
settings-date-future = A data indicada está no futuro
settings-start-rescan = Iniciar nova análise
settings-starting-rescan = A iniciar nova análise...
settings-backup-encrypted-descriptor = Fazer cópia de segurança do descriptor encriptado
settings-backup-encrypted-descriptor-tooltip = Um ficheiro de descriptor encriptado (.bed) que pode guardar em qualquer lugar. Para o desencriptar, precisa de um dos seus dispositivos de assinatura ou xpubs.
settings-wallet-descriptor = Descriptor da carteira:
settings-register-on-device = Registar no hardware wallet
settings-wallet-alias = Alias da carteira:
settings-alias = Alias
settings-alias-too-long = Introduza um alias que não seja demasiado longo
settings-fingerprint-aliases = Aliases de fingerprint:
settings-correct-alias = Introduza um alias correto
settings-updated = Atualizado
settings-update = Atualizar
settings-updating = A atualizar
common-and = e
common-blocks = blocos
policy-signatures = { $count ->
    [one] 1 assinatura
   *[other] {$count} assinaturas
}
policy-out-of-by = de {$count} por
policy-by = por
policy-primary-path = podem sempre gastar os fundos desta carteira (caminho principal)
policy-inactive-for = podem gastar moedas inativas durante
policy-safety-net-path = (Caminho Safety Net)
policy-recovery-path = (Caminho de recuperação n.º {$number})
policy-wallet-policy = A política da carteira:
settings-select-device = Selecionar dispositivo:
common-skip = Ignorar
common-email = Email
common-continue = Continuar
installer-backed-up-mnemonic-show-xpub = Fiz cópia de segurança da mnemonic, mostrar a chave pública alargada
installer-bitcoin-node-management = Gestão do nó Bitcoin
installer-already-have-node = Já tenho um nó
installer-auto-install-node = Quero que a Liana instale automaticamente um nó Bitcoin no meu dispositivo
installer-existing-node-description = Selecione esta opção se já tiver um nó Bitcoin em execução localmente ou remotamente. A Liana irá ligar-se a ele.
installer-managed-node-description = A Liana irá instalar um nó pruned no seu computador. Não terá de fazer nada exceto ter algum espaço em disco disponível (~30 GB necessários em mainnet) e aguardar a sincronização inicial com a rede (pode demorar alguns dias, dependendo da velocidade da sua ligação à internet).
installer-start-bitcoin-node = Iniciar nó completo Bitcoin
installer-download-complete = Transferência concluída
installer-downloading-bitcoin-core = A transferir Bitcoin Core {$version}
installer-download-failed = A transferência falhou: '{$error}'.
installer-installing-bitcoind = A instalar bitcoind...
installer-installation-complete = Instalação concluída
installer-installation-failed = A instalação falhou: '{$error}'.
installer-bitcoind-already-installed = bitcoind gerido pela Liana já instalado
installer-started = Iniciado
installer-starting = A iniciar...
installer-finalize-installation = Finalizar instalação
installer-installing = A instalar...
installer-installed = Instalado
installer-threshold-keys = {$threshold} de {$total} chaves
installer-available-after-inactivity = Disponível após inatividade de ~
installer-able-to-move-any-time = Pode mover os fundos a qualquer momento.
installer-backup-mnemonic-title = Fazer cópia de segurança da sua mnemonic
installer-backed-up-mnemonic-checkbox = Fiz cópia de segurança da minha mnemonic
installer-import-mnemonic-title = Importar Mnemonic
installer-import-mnemonic = Importar mnemonic
installer-choose-backend = Escolher backend
installer-use-own-node = Usar o meu próprio nó
installer-use-liana-connect = Usar Liana Connect
installer-local-wallet-description = Use o seu nó Bitcoin existente ou instale um automaticamente. A carteira Liana não se ligará a nenhum servidor externo.

    Esta é a opção mais privada, mas os dados ficam guardados apenas localmente neste computador. Deve fazer as suas próprias cópias de segurança e partilhar o descriptor com as outras pessoas que quiser que possam aceder à carteira.
installer-remote-backend-description = Use o nosso serviço para ficar imediatamente pronto a transacionar. A Wizardsardine opera a infraestrutura, permitindo que vários computadores ou participantes se liguem e sincronizem.

    Esta é uma opção mais simples e segura para quem quer que a Wizardsardine mantenha uma cópia de segurança do descriptor. Continua a controlar as suas chaves, e a Wizardsardine não tem controlo sobre os seus fundos, mas poderá ver informação da sua carteira associada a um endereço de email. Utilizadores focados em privacidade devem executar a sua própria infraestrutura.
installer-more-backend-node-info = Mais informação sobre opções de backend e nó
installer-choose-existing-account = Escolha uma conta que já utiliza:
installer-enter-wallet-email = Introduza um email que quer associar à carteira:
installer-enter-new-wallet-email = Ou introduza um novo email que quer associar à carteira:
installer-send-token = Enviar token
installer-auth-token-emailed = Foi-lhe enviado um token de autenticação por email
installer-change-email = Alterar email
installer-give-wallet-alias = Dê um alias à sua carteira
installer-wallet-alias = Alias da carteira
installer-change-alias-later = Poderá alterá-lo mais tarde em Definições > Carteira
common-edit = Editar
common-set = Definir
common-apply = Aplicar
common-replace = Substituir
common-retry = Tentar novamente
installer-descriptor-type = Tipo de descriptor
installer-taproot-supported-version = Taproot só é suportado pela Liana versão 5.0 e superior
installer-add-safety-net-key = Adicionar chave Safety Net
installer-add-key = Adicionar chave
installer-keys-inactivity = As chaves podem mover os fundos após inatividade de:
installer-sequence-value-warning = O valor deve ser superior a 0 e inferior a 65535
installer-threshold = Limiar:
installer-key-name-alias = Nome da chave (alias):
installer-key-name-help = Dê um nome amigável a esta chave. Irá ajudá-lo a identificá-la mais tarde:
installer-key-alias-placeholder = Ex.: O Meu Hardware Wallet
installer-key-path-account = Conta do caminho da chave:
installer-key-index = Chave @{$index}:
decrypt-unlock-device = Desbloqueie ou abra a app no dispositivo
decrypt-try-device = Tentar desencriptar com este dispositivo...
decrypt-device-failed = Falha ao desencriptar o ficheiro com este dispositivo
decrypt-device-description = Ligue e desbloqueie um hardware device pertencente a esta configuração para desencriptar automaticamente a cópia de segurança
decrypt-other-options = Outras opções
decrypt-airgap-help = Está a usar um dispositivo air-gapped? Exporte o xpub do dispositivo e use a opção de carregamento ou colagem. Se não souber o caminho de derivação correto, tente com o seguinte:
decrypt-provide-xpub = Forneça um dos xpubs usados nesta carteira.
decrypt-upload-xpub-file = Carregar ficheiro de chave pública alargada
decrypt-pairing-code = Código de emparelhamento: {$code}
decrypt-paste-xpub = Colar uma chave pública alargada
decrypt-enter-mnemonic-unsafe = INSEGURO: Introduzir a mnemonic de uma das chaves
decrypt-enter-mnemonic-warning = Esta opção não é segura. Compreendo que introduzir uma mnemonic num computador pode resultar no roubo dos meus fundos.
decrypt-backup-file = Desencriptar ficheiro de backup
decrypt-invalid-encoding = O ficheiro não pode ser descodificado corretamente; parece não ser um backup encriptado.
decrypt-invalid-type = O ficheiro foi desencriptado, mas o tipo de conteúdo não é suportado.
decrypt-invalid-descriptor = O ficheiro foi desencriptado, mas o descriptor não é um descriptor Liana válido.
installer-introduction = Introdução
installer-build-your-own = Criar a sua própria configuração
installer-custom-template-description-1 = Para esta configuração terá de definir as suas políticas de gasto principal e de recuperação. Por motivos de segurança, sugerimos que use um Hardware Wallet separado para cada chave pertencente a elas.
installer-custom-template-description-2 = As chaves da sua política principal podem sempre gastar. As chaves das políticas de recuperação só poderão gastar após um tempo definido de inatividade da carteira, permitindo recuperação segura e políticas de gasto avançadas.
installer-primary-spending-option = Opção de gasto principal:
installer-primary-key = Chave principal
installer-recovery-option = Opção de recuperação n.º {$number}:
installer-recovery-key = Chave de recuperação
installer-add-recovery-option = Adicionar opção de recuperação
installer-add-safety-net = Adicionar Safety Net
installer-safety-net-description = Isto adiciona uma opção final de recuperação com chaves de agentes profissionais.

    Use esta opção se lhe tiverem sido fornecidos um ou mais tokens Safety Net.
installer-safety-net = Safety Net:
installer-safety-net-key = Chave Safety Net
installer-set-keys = Definir chaves
installer-plug-hardware-device = Ligue um hardware device ...
installer-detected-hardware = Hardware detetado
installer-no-other-sources = - Nenhuma outra fonte detetada -
installer-already-used-sources = Fontes já usadas
installer-advanced-settings = Definições avançadas
common-clear-all = Limpar tudo
installer-customize = Personalizar
installer-choose-wallet-type = Escolher tipo de carteira
installer-simple-inheritance = Herança simples
installer-simple-inheritance-description = São necessárias duas chaves, uma para si gastar e outra para o seu herdeiro.
installer-expanding-multisig = Multisig expansível
installer-expanding-multisig-description = São necessárias duas chaves para gastar, com uma chave extra como backup.
installer-build-your-own-description = Crie uma configuração personalizada que responda às suas necessidades.
installer-simple-inheritance-wallet = Carteira de herança simples
installer-inheritance-description-1 = Para esta configuração precisa de 2 chaves: a sua chave principal (para si) e uma chave de herança (para o seu herdeiro). Por motivos de segurança, sugerimos que use um Hardware Wallet separado para cada chave.
installer-inheritance-key = Chave de herança
installer-inheritance-description-2 = Poderá sempre gastar usando a sua chave principal. Após um período de inatividade (mas não antes), a sua chave de herança poderá recuperar os seus fundos.
installer-device-no-taproot = Este dispositivo não suporta Taproot
installer-expanding-multisig-wallet = Carteira multisig expansível
installer-multisig-description-1 = Para esta configuração precisa de 3 chaves: duas chaves principais e uma chave de recuperação. Por motivos de segurança, sugerimos que use um Hardware Wallet separado para cada chave.
installer-primary-key-number = Chave principal n.º {$number}
installer-multisig-description-2 = As chaves principais formam uma multisig 2-de-2 que poderá sempre gastar. Se uma das suas chaves ficar indisponível, após um período de inatividade poderá recuperar os seus fundos usando a chave de recuperação juntamente com uma das suas chaves principais (multisig 2-de-3):
installer-key-source-no-taproot = Esta origem de chave não suporta Taproot
common-update = Atualizar
psbt-transaction-saved = Transação guardada
psbt-save-transaction = Guardar esta transação
psbt-transaction-broadcast = Transação transmitida
psbt-broadcast-transaction = Transmitir a transação
psbt-broadcast = Transmitir
psbt-broadcast-invalidates-some = AVISO: Transmitir esta transação invalidará alguns pagamentos pendentes.
psbt-broadcast-invalidates-one = AVISO: Transmitir esta transação invalidará um pagamento pendente.
psbt-broadcast-conflicts-some = As seguintes transações estão a gastar uma ou mais entradas da transação a transmitir e serão removidas, juntamente com quaisquer outras transações que dependam delas:
psbt-broadcast-conflicts-one = A seguinte transação está a gastar uma ou mais entradas da transação a transmitir e será removida, juntamente com quaisquer outras transações que dependam dela:
psbt-delete-success = Transação eliminada com sucesso.
psbt-go-back = Voltar aos PSBTs
psbt-delete-this = Eliminar este PSBT
psbt-missing-inputs = Falta informação sobre as entradas da transação
psbt-sign-save-before-export = Assine ou guarde a transação primeiro para ativar a exportação
psbt-sign = Assinar
psbt-status = Estado
psbt-ready = Pronto
psbt-signed-by = assinado por
psbt-not-ready = Não está pronto
psbt-finalizing-requires = Finalizar esta transação requer:
psbt-more-signatures = { $count ->
    [0] mais nenhuma assinatura
    [one] mais 1 assinatura de
   *[other] mais {$count} assinaturas de
}
psbt-already-signed-by = , já assinado por
psbt-coins-spent = { $count ->
    [one] 1 moeda gasta
   *[other] {$count} moedas gastas
}
psbt-payments = { $count ->
    [one] 1 pagamento
   *[other] {$count} pagamentos
}
psbt-no-payment = 0 pagamentos
psbt-change = Troco
psbt-select-signing-device = Selecionar dispositivo de assinatura:
psbt-device-sign-failed = O dispositivo falhou ao assinar
psbt-label = PSBT:
psbt-insert-updated = Inserir PSBT atualizado:
psbt-base64-correct-warning = Introduza o PSBT codificado em base64 correto
psbt-spend-updated = Transação de gasto atualizada
common-back = Voltar
payment-outgoing = Pagamento enviado
payment-incoming = Pagamento recebido
payment-title = Pagamento
payment-see-transaction-details = Ver detalhes da transação
label-add = Adicionar etiqueta
label-label = Etiqueta
label-invalid-length = Comprimento de etiqueta inválido, não pode ser superior a 100
loader-starting-daemon = A iniciar daemon...
loader-connecting-daemon = A ligar ao daemon...
loader-progress = Progresso {$progress}%
loader-sync-progress-1 = O Bitcoin Core está a sincronizar a blockchain. Uma sincronização completa demora normalmente alguns dias e consome muitos recursos. Depois da sincronização inicial, as seguintes serão muito mais rápidas.
loader-sync-progress-2 = O Bitcoin Core está a sincronizar a blockchain. Isto demorará algum tempo, dependendo da última vez que foi feito, da sua ligação à internet e do desempenho do computador.
loader-sync-progress-3 = O Bitcoin Core está a sincronizar a blockchain. Isto pode demorar alguns minutos, dependendo da última vez que foi feito, da sua ligação à internet e do desempenho do computador.
loader-failed-bitcoind = A Liana não conseguiu iniciar; verifique se o bitcoind está em execução
loader-failed = A Liana não conseguiu iniciar
business-common-you = Você
business-edited-by =  por {$name}
business-edited-relative = Editado{$editor} {$time}
business-login-email-help = Introduza o email associado à sua conta
business-select-account = Selecione uma conta para continuar
business-connect-another-email = Ligar com outro email
installer-auth-token-emailed-to = Foi enviado um token de autenticação por email para
business-wallet-count = { $count ->
    [one] (1 carteira)
   *[other] ({$count} carteiras)
}
business-key-count = { $count ->
    [one] (1 chave)
   *[other] ({$count} chaves)
}
business-contact-create-account = Contacte a WizardSardine para criar uma conta.
business-no-orgs-search = Nenhuma organização encontrada para a sua pesquisa.
business-organizations = Organizações
business-select-organization = Selecionar uma organização
business-filter-organizations = Filtrar organizações...
business-organization = Organização
business-wallet = Carteira
business-wallets = Carteiras
business-select-wallet = Selecionar carteira
business-create-wallet = Criar uma carteira
business-filter-wallets = Filtrar carteiras...
business-no-wallets-search = Nenhuma carteira encontrada para a sua pesquisa.
business-role-admin = Admin
business-role-manager = Gestor
business-role-participant = Participante
business-keys = Chaves
business-keys-instruction = Adicione as chaves que farão parte desta carteira e associe cada uma ao email do seu proprietário.
business-add-key = + Adicionar uma chave
business-unable-load-wallet = Não foi possível carregar a carteira
business-service-unavailable = O serviço está temporariamente indisponível. Os dados da sua carteira e os fundos não foram afetados.
business-try-again-support = Tente novamente em breve. Se o problema persistir, contacte o suporte.
business-loading-wallet = A carregar carteira...
business-manage-keys = Gerir chaves
business-send-for-approval = Enviar para aprovação
business-unlock = Desbloquear
business-approve-template = Aprovar template
business-template = Template
business-set-keys = Definir chaves
business-your-key-set = A sua chave está definida.
business-your-keys-set = As suas chaves estão definidas.
business-wait-other-key-setup = Assim que os outros participantes concluírem a configuração das chaves, poderá aceder à carteira.
business-xpub-instruction = Selecione uma chave para concluir a sua configuração. As chaves podem ser configuradas individualmente por cada gestor de chave, ou pelo gestor da carteira em nome deles. Pode ligar um hardware device (recomendado) ou adicionar manualmente uma chave pública alargada (xpub).
business-wallet-set-keys = {$wallet} - Definir chaves
business-no-keys-assigned = Não há chaves atribuídas a si
business-no-keys-found = Nenhuma chave encontrada
business-your-keys = As suas chaves:
business-other-participants-keys = Chaves dos outros participantes:
business-register-devices = Registar dispositivos
business-register-wallet-devices = Registar carteira nos dispositivos
business-register-wallet-devices-help = Registe o descriptor da carteira em cada dispositivo, ou ignore se não estiver disponível.
business-no-devices-register = Não há dispositivos para registar
business-no-devices-assigned = Não tem dispositivos atribuídos nesta carteira.
business-register = Registar
business-device-unsupported-locked = Dispositivo não suportado ou bloqueado
business-connect-device-register = Ligue o dispositivo associado para registar
business-xpub-already-set-help = Esta chave já tem um xpub. Pode substituí-lo obtendo-o de um dispositivo, importando de um ficheiro ou colando. Use o botão Limpar para o remover completamente.
business-current-xpub = Xpub atual:
business-select-key-source = Selecionar origem da chave - {$alias}
business-fetching-device = A obter do dispositivo...
business-account-number = Conta n.º {$index}
business-no-hardware-wallets = Nenhum hardware wallet detetado. Ligue um dispositivo e desbloqueie-o.
business-detected-devices = Dispositivos detetados:
business-unlock-device = Desbloqueie o dispositivo
business-not-part-wallet = Não faz parte desta carteira (#{$fingerprint})
business-wrong-network-device = Rede errada nas definições do dispositivo
business-device-version-unsupported = Versão do dispositivo não suportada, atualize para uma versão > {$version}
business-unsupported-method = Método não suportado: {$method}
business-open-app-device = Abra a app no dispositivo
business-import-xpub-file = Importar ficheiro de chave pública alargada
business-edit-primary-path = Editar caminho principal
business-edit-recovery-path = Editar caminho de recuperação
business-create-new-path = Criar novo caminho
business-keys-in-path = Chaves no caminho:
business-no-keys-available = Não há chaves disponíveis. Adicione chaves primeiro.
business-key-number = Chave {$id}
business-invalid-threshold = Valor de limiar inválido
business-threshold-range = Limiar (1-{$count}):
business-timelock-zero = O timelock não pode ser zero
business-max-unit = Máx. {$max} {$unit}
business-duplicate-timelock = Timelock duplicado
business-timelock = Timelock:
business-max-unit-label = Máx.: {$max} {$unit}
business-no-timelock = Sem timelock
business-after-months = { $count ->
    [one] Após 1 mês
   *[other] Após {$count} meses
}
business-after-days = { $count ->
    [one] Após 1 dia
   *[other] Após {$count} dias
}
business-after-hours = { $count ->
    [one] Após 1 hora
   *[other] Após {$count} horas
}
business-no-keys = Sem chaves
business-all-of = Todas de {$names}
business-threshold-of = {$threshold} de {$names}
business-spendable-anytime = Gastável a qualquer momento
business-add-recovery-path = + Adicionar caminho de recuperação
business-confirm-device = Confirme no seu dispositivo...
business-registering-wallet = A registar carteira
business-registration-failed = Registo falhou
business-confirm-coldcard-success = Confirme na sua Coldcard que o registo da carteira foi concluído com sucesso.
business-did-registration-succeed = O registo foi bem-sucedido na sua Coldcard?
business-confirm-registration = Confirmar registo
business-keep-my-changes = Manter as minhas alterações
common-reload = Recarregar
business-new-key = Nova chave
business-edit-key = Editar chave
business-key-alias = Alias da chave
business-enter-key-alias = Introduzir alias da chave
business-key-type = Tipo de chave
business-key-type-tooltip = Internal: chaves detidas pela sua organização.
    External: chaves detidas por terceiros.
    Cosigner: chave profissional de coassinatura de terceiro.
    SafetyNet: chave profissional de recuperação de terceiro.
business-key-manager-email = Email do gestor da chave
business-enter-email-address = Introduzir endereço de email
business-enter-token-placeholder = Introduzir token (ex.: 42-absent-cake-eagle)
business-authenticated = Autenticado
business-connection-failed = Ligação falhou
business-user-session-not-found = Sessão de utilizador não encontrada. Inicie sessão novamente ou contacte a WizardSardine.
business-access-error = Erro de acesso
business-wallet-access-denied = Não tem acesso a esta carteira. Contacte a WizardSardine.
business-backend-error = Erro do backend
business-connection-error = Erro de ligação
business-lost-connection-restart = Perdeu-se a ligação ao servidor. Reinicie a aplicação.
business-account-connection-failed = Falha ao ligar com a conta {$email}. A sessão pode ter expirado.
business-key-deleted = Chave eliminada
business-key-deleted-message = A chave que estava a editar foi eliminada por outro utilizador.
business-key-modified = Chave modificada
business-key-modified-message = Esta chave foi modificada por outro utilizador. Quer recarregar a versão do servidor ou manter as suas alterações?
business-key-removed = Chave removida
business-key-removed-from-path = "{$alias}" foi eliminada por outro utilizador e removida da sua seleção de caminho.
business-path-modified = Caminho modificado
business-primary-path-modified-message = O caminho principal foi modificado por outro utilizador. Quer recarregar a versão do servidor ou manter as suas alterações?
business-path-deleted = Caminho eliminado
business-path-deleted-message = O caminho que estava a editar foi eliminado por outro utilizador.
business-recovery-path-modified-message = Este caminho de recuperação foi modificado por outro utilizador. Quer recarregar a versão do servidor ou manter as suas alterações?
business-device-locked-unlock = O dispositivo está bloqueado. Desbloqueie-o primeiro.
business-device-not-supported = Dispositivo não suportado
business-hardware-wallet-not-found = Hardware wallet não encontrada
business-select-xpub-file = Selecionar ficheiro xpub
business-text-files = Ficheiros de texto
business-all-files = Todos os ficheiros
business-file-read-failed = Falha ao ler ficheiro: {$error}
business-file-dialog-result-failed = Falha ao receber resultado da janela de ficheiros
business-clipboard-empty = A área de transferência está vazia
business-no-descriptor-available = Nenhum descriptor disponível
business-no-wallet-selected = Nenhuma carteira selecionada
business-no-user-id-available = Nenhum ID de utilizador disponível
business-auth-code-request-failed = Falha ao pedir código de autenticação ao servidor.
business-login-failed = Início de sessão falhou.
business-xpub-empty = A chave pública alargada não pode estar vazia.
business-xpub-invalid-format = Formato de chave pública alargada inválido: {$error}
business-xpub-invalid-network = A chave pública alargada não é válida para {$network}.
business-device-disconnected = Dispositivo desligado
business-token-invalid = Token inválido.
business-token-duplicate = Token duplicado.
business-code-six-digits = O código só pode conter 6 dígitos.
business-admin-name = Admin{$name}
time-just-now = agora mesmo
time-minutes-ago = { $count ->
    [one] há 1 minuto
   *[other] há {$count} minutos
}
time-hours-ago = { $count ->
    [one] há 1 hora
   *[other] há {$count} horas
}
time-days-ago = { $count ->
    [one] há 1 dia
   *[other] há {$count} dias
}
time-weeks-ago = { $count ->
    [one] há 1 semana
   *[other] há {$count} semanas
}
time-months-ago = { $count ->
    [one] há 1 mês
   *[other] há {$count} meses
}
error-unknown = Erro desconhecido
warning-wallet-error = Erro da carteira
warning-fields-invalid = Alguns campos são inválidos
warning-internal-error = Erro interno
warning-http-code-error = Erro HTTP {$code}: {$error}
warning-http-error = Erro HTTP: {$error}
warning-daemon-start-failed = Falha ao iniciar o daemon
warning-daemon-client-unsupported = Cliente do daemon não suportado
warning-daemon-communication-failed = Falha na comunicação com o daemon
warning-daemon-stopped = Daemon parado
warning-coin-selection-error = Erro ao selecionar coins para gastar
warning-backend-feature-unimplemented = Funcionalidade não implementada para este backend
warning-hardware-wallet-error = Erro de hardware wallet
warning-descriptor-analysis-error = Erro de análise do descriptor: '{$error}'.
warning-spend-creation-error = Erro ao criar gasto: '{$error}'.
warning-restore-backup-failed = Falha ao restaurar backup: {$error}
warning-fiat-price-error = Erro de preço fiat: {$error}
common-ok = OK
common-yes = Sim
common-no = Não
common-reset-timelock = Repor timelock
common-go-to-rescan = Ir para rescan
common-dismiss = Dispensar
pill-recovery = Recuperação
pill-recovery-tooltip = Esta transação usa um caminho de recuperação
pill-batch = Lote
pill-batch-tooltip = Esta transação contém vários pagamentos
pill-deprecated = Obsoleta
pill-deprecated-tooltip = Esta transação já não pode ser incluída na blockchain.
pill-spent = Gasta
pill-spent-tooltip = A transação foi incluída na blockchain.
pill-unsigned = Não assinada
pill-unsigned-tooltip = Faltam assinatura(s) nesta transação
pill-signed = Para difundir
pill-signed-tooltip = Esta transação está assinada e pronta para difusão
pill-unconfirmed = Não confirmada
pill-unconfirmed-tooltip = Não trate isto como um pagamento até estar confirmado
pill-confirmed = Confirmada
pill-confirmed-tooltip = Esta transação foi incluída num bloco
pill-key-internal = Internal
pill-key-internal-tooltip = Chave detida pela sua organização
pill-key-external = External
pill-key-external-tooltip = Chave detida por terceiros
pill-key-cosigner = Cosigner
pill-key-cosigner-tooltip = Chave profissional de coassinatura de terceiro
pill-key-safety-net = Safety Net
pill-key-safety-net-tooltip = Chave profissional de recuperação de terceiro
pill-to-approve = A aprovar
pill-draft = Rascunho
pill-set-keys = Definir chaves
pill-active = Ativa
pill-ws-admin = WS Admin
pill-register = Registar
pill-xpub-set = ✓ Definida
pill-xpub-not-set = Não definida
pill-rescan-progress = Rescan… {$progress}%
pill-available = Disponível
pill-today = Hoje
pill-recovery-available-tooltip = Opção/opções de recuperação já disponíveis
pill-first-recovery-today = Primeira opção de recuperação disponível hoje
pill-first-recovery-in = Primeira opção de recuperação disponível em {$units}
duration-years = { $count ->
    [one] 1 ano
   *[other] {$count} anos
}
duration-months = { $count ->
    [one] 1 mês
   *[other] {$count} meses
}
duration-days = { $count ->
    [one] 1 dia
   *[other] {$count} dias
}
duration-days-approx = ~{$count} dias
duration-hours = { $count ->
    [one] 1 hora
   *[other] {$count} horas
}
duration-minutes = { $count ->
    [one] 1 minuto
   *[other] {$count} minutos
}
