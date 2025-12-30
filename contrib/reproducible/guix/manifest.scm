(use-modules
  (gnu packages llvm)
  (gnu packages rust)
  (gnu packages zig)
  (gnu packages pkg-config)
  (gnu packages compression)
  (gnu packages tls)
  (gnu packages base)
  (guix build-system cargo)
  ((guix licenses) #:prefix license:)
  (guix download)
  (guix packages)
  (guix utils))


(define rust-anstream-0.6.18
  (crate-source "anstream" "0.6.18"
                "16sjk4x3ns2c3ya1x28a44kh6p47c7vhk27251i015hik1lm7k4a"))

(define rust-anstyle-1.0.10
  (crate-source "anstyle" "1.0.10"
                "1yai2vppmd7zlvlrp9grwll60knrmscalf8l2qpfz8b7y5lkpk2m"))

(define rust-anstyle-parse-0.2.6
  (crate-source "anstyle-parse" "0.2.6"
                "1acqayy22fwzsrvr6n0lz6a4zvjjcvgr5sm941m7m0b2fr81cb9v"))

(define rust-anstyle-query-1.1.2
  (crate-source "anstyle-query" "1.1.2"
                "036nm3lkyk43xbps1yql3583fp4hg3b1600is7mcyxs1gzrpm53r"))

(define rust-anstyle-wincon-3.0.7
  (crate-source "anstyle-wincon" "3.0.7"
                "0kmf0fq4c8yribdpdpylzz1zccpy84hizmcsac3wrac1f7kk8dfa"))

(define rust-anyhow-1.0.98
  (crate-source "anyhow" "1.0.98"
                "11ylvjdrcjs0q9jgk1af4r5cx1qppj63plxqkq595vmc24rjsvg1"))

(define rust-autocfg-1.4.0
  (crate-source "autocfg" "1.4.0"
                "09lz3by90d2hphbq56znag9v87gfpd9gb8nr82hll8z6x2nhprdc"))

(define rust-bitflags-2.8.0
  (crate-source "bitflags" "2.8.0"
                "0dixc6168i98652jxf0z9nbyn0zcis3g6hi6qdr7z5dbhcygas4g"))

(define rust-camino-1.1.9
  (crate-source "camino" "1.1.9"
                "1lqszl12l1146jf8g01rvjmapif82mhzih870ln3x0dmcr4yr5lb"))

(define rust-cargo-config2-0.1.32
  (crate-source "cargo-config2" "0.1.32"
                "0qf4kkbh3m47n6s3scaqjr41ysn3n153wyhy3yckqhp06sd79hvd"))

(define rust-cargo-metadata-0.19.0
  (crate-source "cargo_metadata" "0.19.0"
                "11gb6kf93ajz6diy3hv6g22xp578dzsiif0gqmbqjv27i7nhkhxg"))

(define rust-cargo-options-0.7.5
  (crate-source "cargo-options" "0.7.5"
                "0wc1qy1plwp6i0g5p74bnvy545xk7hccvv68lmmg4739g0ay923l"))

(define rust-cargo-platform-0.1.6
  (crate-source "cargo-platform" "0.1.6"
                "0ga4qa3fx4bidnmix5gl8qclx2mma1a441swlpfsa645kpv8xvff"))

(define rust-cfg-if-1.0.0
  (crate-source "cfg-if" "1.0.0"
                "1za0vb97n4brpzpv8lsbnzmq5r8f2b0cpqqr0sy8h5bn751xxwds"))

(define rust-clap-4.5.28
  (crate-source "clap" "4.5.28"
                "1zq53kp3lfcz9xr584i7r9bw8ivkcra53jvj6v046hnr7cjc6xry"))

(define rust-clap-builder-4.5.27
  (crate-source "clap_builder" "4.5.27"
                "1mys7v60lys8zkwpk49wif9qnja9zamm4dnrsbj40wdmni78h9hv"))

(define rust-clap-derive-4.5.28
  (crate-source "clap_derive" "4.5.28"
                "1vgigkhljp3r8r5lwdrn1ij93nafmjwh8cx77nppb9plqsaysk5z"))

(define rust-clap-lex-0.7.4
  (crate-source "clap_lex" "0.7.4"
                "19nwfls5db269js5n822vkc8dw0wjq2h1wf0hgr06ld2g52d2spl"))

(define rust-colorchoice-1.0.3
  (crate-source "colorchoice" "1.0.3"
                "1439m3r3jy3xqck8aa13q658visn71ki76qa93cy55wkmalwlqsv"))

(define rust-crc-3.2.1
  (crate-source "crc" "3.2.1"
                "0dnn23x68qakzc429s1y9k9y3g8fn5v9jwi63jcz151sngby9rk9"))

(define rust-crc-catalog-2.4.0
  (crate-source "crc-catalog" "2.4.0"
                "1xg7sz82w3nxp1jfn425fvn1clvbzb3zgblmxsyqpys0dckp9lqr"))

(define rust-dirs-5.0.1
  (crate-source "dirs" "5.0.1"
                "0992xk5vx75b2x91nw9ssb51mpl8x73j9rxmpi96cryn0ffmmi24"))

(define rust-dirs-sys-0.4.1
  ;; TODO: Check bundled sources.
  (crate-source "dirs-sys" "0.4.1"
                "071jy0pvaad9lsa6mzawxrh7cmr7hsmsdxwzm7jzldfkrfjha3sj"))

(define rust-either-1.13.0
  (crate-source "either" "1.13.0"
                "1w2c1mybrd7vljyxk77y9f4w9dyjrmp3yp82mk7bcm8848fazcb0"))

(define rust-env-home-0.1.0
  (crate-source "env_home" "0.1.0"
                "1zn08mk95rjh97831rky1n944k024qrwjhbcgb0xv9zhrh94xy67"))

(define rust-equivalent-1.0.1
  (crate-source "equivalent" "1.0.1"
                "1malmx5f4lkfvqasz319lq6gb3ddg19yzf9s8cykfsgzdmyq0hsl"))

(define rust-errno-0.3.10
  (crate-source "errno" "0.3.10"
                "0pgblicz1kjz9wa9m0sghkhh2zw1fhq1mxzj7ndjm746kg5m5n1k"))

(define rust-fat-macho-0.4.9
  (crate-source "fat-macho" "0.4.9"
                "0idkn366wipv2l757yqfgzgibqc6jvm89gdk9kpgmvf6lv54b72c"))

(define rust-fs-err-3.1.0
  (crate-source "fs-err" "3.1.0"
                "1al2sj8src02wwww70vv2gypsrs6wyzx6zlpk82h84m2qajbv28z"))

(define rust-getrandom-0.2.15
  (crate-source "getrandom" "0.2.15"
                "1mzlnrb3dgyd1fb84gvw10pyr8wdqdl4ry4sr64i1s8an66pqmn4"))

(define rust-goblin-0.9.3
  (crate-source "goblin" "0.9.3"
                "0ifpcsp0hpp7lx10yqln9ybmfkky7gig9idlhc2j7sx7456sd86s"))

(define rust-hashbrown-0.15.2
  (crate-source "hashbrown" "0.15.2"
                "12dj0yfn59p3kh3679ac0w1fagvzf4z2zp87a13gbbqbzw0185dz"))

(define rust-heck-0.5.0
  (crate-source "heck" "0.5.0"
                "1sjmpsdl8czyh9ywl3qcsfsq9a307dg4ni2vnlwgnzzqhc4y0113"))

(define rust-indexmap-2.7.1
  (crate-source "indexmap" "2.7.1"
                "0lmnm1zbr5gq3wic3d8a76gpvampridzwckfl97ckd5m08mrk74c"))

(define rust-is-terminal-polyfill-1.70.1
  (crate-source "is_terminal_polyfill" "1.70.1"
                "1kwfgglh91z33kl0w5i338mfpa3zs0hidq5j4ny4rmjwrikchhvr"))

(define rust-itoa-1.0.14
  (crate-source "itoa" "1.0.14"
                "0x26kr9m062mafaxgcf2p6h2x7cmixm0zw95aipzn2hr3d5jlnnp"))

(define rust-libc-0.2.169
  (crate-source "libc" "0.2.169"
                "02m253hs8gw0m1n8iyrsc4n15yzbqwhddi7w1l0ds7i92kdsiaxm"))

(define rust-libredox-0.1.3
  (crate-source "libredox" "0.1.3"
                "139602gzgs0k91zb7dvgj1qh4ynb8g1lbxsswdim18hcb6ykgzy0"))

(define rust-linux-raw-sys-0.4.15
  ;; TODO: Check bundled sources.
  (crate-source "linux-raw-sys" "0.4.15"
                "1aq7r2g7786hyxhv40spzf2nhag5xbw2axxc1k8z5k1dsgdm4v6j"))

(define rust-log-0.4.25
  (crate-source "log" "0.4.25"
                "17ydv5zhfv1zzygy458bmg3f3jx1vfziv9d74817w76yhfqgbjq4"))

(define rust-memchr-2.7.4
  (crate-source "memchr" "2.7.4"
                "18z32bhxrax0fnjikv475z7ii718hq457qwmaryixfxsl2qrmjkq"))

(define rust-once-cell-1.20.2
  (crate-source "once_cell" "1.20.2"
                "0xb7rw1aqr7pa4z3b00y7786gyf8awx2gca3md73afy76dzgwq8j"))

(define rust-option-ext-0.2.0
  (crate-source "option-ext" "0.2.0"
                "0zbf7cx8ib99frnlanpyikm1bx8qn8x602sw1n7bg6p9x94lyx04"))

(define rust-path-slash-0.2.1
  (crate-source "path-slash" "0.2.1"
                "0hjgljv4vy97qqw9gxnwzqhhpysjss2yhdphfccy3c388afhk48y"))

(define rust-plain-0.2.3
  (crate-source "plain" "0.2.3"
                "19n1xbxb4wa7w891268bzf6cbwq4qvdb86bik1z129qb0xnnnndl"))

(define rust-proc-macro2-1.0.93
  (crate-source "proc-macro2" "1.0.93"
                "169dw9wch753if1mgyi2nfl1il77gslvh6y2q46qplprwml6m530"))

(define rust-quote-1.0.38
  (crate-source "quote" "1.0.38"
                "1k0s75w61k6ch0rs263r4j69b7vj1wadqgb9dia4ylc9mymcqk8f"))

(define rust-redox-users-0.4.6
  (crate-source "redox_users" "0.4.6"
                "0hya2cxx6hxmjfxzv9n8rjl5igpychav7zfi1f81pz6i4krry05s"))

(define rust-rustc-version-0.4.1
  (crate-source "rustc_version" "0.4.1"
                "14lvdsmr5si5qbqzrajgb6vfn69k0sfygrvfvr2mps26xwi3mjyg"))

(define rust-rustflags-0.1.6
  (crate-source "rustflags" "0.1.6"
                "1h1al0xhd9kzy8q8lzw6rxip5zjifxigfrm3blf462mmkwar5z6p"))

(define rust-rustix-0.38.44
  (crate-source "rustix" "0.38.44"
                "0m61v0h15lf5rrnbjhcb9306bgqrhskrqv7i1n0939dsw8dbrdgx"))

(define rust-ryu-1.0.19
  (crate-source "ryu" "1.0.19"
                "1pg6a0b80m32ahygsdkwzs3bfydk4snw695akz4rqxj4lv8a58bf"))

(define rust-scroll-0.12.0
  (crate-source "scroll" "0.12.0"
                "19mix9vm4k23jkknpgbi0ylmhpf2hnlpzzrfj9wqcj88lj55kf3a"))

(define rust-scroll-derive-0.12.0
  (crate-source "scroll_derive" "0.12.0"
                "0cmr3hxk318s2ivv37cik2l1r0d8r0qhahnin5lpxbr5w3yw50bz"))

(define rust-semver-1.0.26
  (crate-source "semver" "1.0.26"
                "1l5q2vb8fjkby657kdyfpvv40x2i2xqq9bg57pxqakfj92fgmrjn"))

(define rust-serde-1.0.219
  (crate-source "serde" "1.0.219"
                "1dl6nyxnsi82a197sd752128a4avm6mxnscywas1jq30srp2q3jz"))

(define rust-serde-derive-1.0.219
  (crate-source "serde_derive" "1.0.219"
                "001azhjmj7ya52pmfiw4ppxm16nd44y15j2pf5gkcwrcgz7pc0jv"))

(define rust-serde-json-1.0.140
  (crate-source "serde_json" "1.0.140"
                "0wwkp4vc20r87081ihj3vpyz5qf7wqkqipq17v99nv6wjrp8n1i0"))

(define rust-serde-spanned-0.6.8
  (crate-source "serde_spanned" "0.6.8"
                "1q89g70azwi4ybilz5jb8prfpa575165lmrffd49vmcf76qpqq47"))

(define rust-shlex-1.3.0
  (crate-source "shlex" "1.3.0"
                "0r1y6bv26c1scpxvhg2cabimrmwgbp4p3wy6syj9n0c4s3q2znhg"))

(define rust-strsim-0.11.1
  (crate-source "strsim" "0.11.1"
                "0kzvqlw8hxqb7y598w1s0hxlnmi84sg5vsipp3yg5na5d1rvba3x"))

(define rust-syn-2.0.98
  (crate-source "syn" "2.0.98"
                "1cfk0qqbl4fbr3dz61nw21d5amvl4rym6nxwnfsw43mf90d7y51n"))

(define rust-target-lexicon-0.13.1
  (crate-source "target-lexicon" "0.13.1"
                "0xrab0br65gd3ws2hgkma0sbf205scpzfdbi1cg3k7cv3jd964nw"))

(define rust-terminal-size-0.4.1
  (crate-source "terminal_size" "0.4.1"
                "1sd4nq55h9sjirkx0138zx711ddxq1k1a45lc77ninhzj9zl8ljk"))

(define rust-thiserror-1.0.69
  (crate-source "thiserror" "1.0.69"
                "0lizjay08agcr5hs9yfzzj6axs53a2rgx070a1dsi3jpkcrzbamn"))

(define rust-thiserror-impl-1.0.69
  (crate-source "thiserror-impl" "1.0.69"
                "1h84fmn2nai41cxbhk6pqf46bxqq1b344v8yz089w1chzi76rvjg"))

(define rust-toml-datetime-0.6.8
  (crate-source "toml_datetime" "0.6.8"
                "0hgv7v9g35d7y9r2afic58jvlwnf73vgd1mz2k8gihlgrf73bmqd"))

(define rust-toml-edit-0.22.23
  (crate-source "toml_edit" "0.22.23"
                "1vhvransgx1ksmdzbr1k3h1xjgs5wfp8k9315n7c3mx3s5rb9a02"))

(define rust-unicode-ident-1.0.16
  (crate-source "unicode-ident" "1.0.16"
                "0d2hji0i16naw43l02dplrz8fbv625n7475s463iqw4by1hd2452"))

(define rust-utf8parse-0.2.2
  (crate-source "utf8parse" "0.2.2"
                "088807qwjq46azicqwbhlmzwrbkz7l4hpw43sdkdyyk524vdxaq6"))

(define rust-wasi-0.11.0+wasi-snapshot-preview1
  (crate-source "wasi" "0.11.0+wasi-snapshot-preview1"
                "08z4hxwkpdpalxjps1ai9y7ihin26y9f476i53dv98v45gkqg3cw"))

(define rust-which-7.0.1
  (crate-source "which" "7.0.1"
                "0a2hvxcyx7c0gijny8l9w9462piqnchnxqxh88bdqfc3chrrwjpv"))

(define rust-windows-aarch64-gnullvm-0.48.5
  (crate-source "windows_aarch64_gnullvm" "0.48.5"
                "1n05v7qblg1ci3i567inc7xrkmywczxrs1z3lj3rkkxw18py6f1b"))

(define rust-windows-aarch64-gnullvm-0.52.6
  (crate-source "windows_aarch64_gnullvm" "0.52.6"
                "1lrcq38cr2arvmz19v32qaggvj8bh1640mdm9c2fr877h0hn591j"))

(define rust-windows-aarch64-msvc-0.48.5
  (crate-source "windows_aarch64_msvc" "0.48.5"
                "1g5l4ry968p73g6bg6jgyvy9lb8fyhcs54067yzxpcpkf44k2dfw"))

(define rust-windows-aarch64-msvc-0.52.6
  (crate-source "windows_aarch64_msvc" "0.52.6"
                "0sfl0nysnz32yyfh773hpi49b1q700ah6y7sacmjbqjjn5xjmv09"))

(define rust-windows-i686-gnu-0.48.5
  (crate-source "windows_i686_gnu" "0.48.5"
                "0gklnglwd9ilqx7ac3cn8hbhkraqisd0n83jxzf9837nvvkiand7"))

(define rust-windows-i686-gnu-0.52.6
  (crate-source "windows_i686_gnu" "0.52.6"
                "02zspglbykh1jh9pi7gn8g1f97jh1rrccni9ivmrfbl0mgamm6wf"))

(define rust-windows-i686-gnullvm-0.52.6
  (crate-source "windows_i686_gnullvm" "0.52.6"
                "0rpdx1537mw6slcpqa0rm3qixmsb79nbhqy5fsm3q2q9ik9m5vhf"))

(define rust-windows-i686-msvc-0.48.5
  (crate-source "windows_i686_msvc" "0.48.5"
                "01m4rik437dl9rdf0ndnm2syh10hizvq0dajdkv2fjqcywrw4mcg"))

(define rust-windows-i686-msvc-0.52.6
  (crate-source "windows_i686_msvc" "0.52.6"
                "0rkcqmp4zzmfvrrrx01260q3xkpzi6fzi2x2pgdcdry50ny4h294"))

(define rust-windows-sys-0.48.0
  ;; TODO: Check bundled sources.
  (crate-source "windows-sys" "0.48.0"
                "1aan23v5gs7gya1lc46hqn9mdh8yph3fhxmhxlw36pn6pqc28zb7"))

(define rust-windows-sys-0.59.0
  ;; TODO: Check bundled sources.
  (crate-source "windows-sys" "0.59.0"
                "0fw5672ziw8b3zpmnbp9pdv1famk74f1l9fcbc3zsrzdg56vqf0y"))

(define rust-windows-targets-0.48.5
  (crate-source "windows-targets" "0.48.5"
                "034ljxqshifs1lan89xwpcy1hp0lhdh4b5n0d2z4fwjx2piacbws"))

(define rust-windows-targets-0.52.6
  (crate-source "windows-targets" "0.52.6"
                "0wwrx625nwlfp7k93r2rra568gad1mwd888h1jwnl0vfg5r4ywlv"))

(define rust-windows-x86-64-gnu-0.48.5
  (crate-source "windows_x86_64_gnu" "0.48.5"
                "13kiqqcvz2vnyxzydjh73hwgigsdr2z1xpzx313kxll34nyhmm2k"))

(define rust-windows-x86-64-gnu-0.52.6
  (crate-source "windows_x86_64_gnu" "0.52.6"
                "0y0sifqcb56a56mvn7xjgs8g43p33mfqkd8wj1yhrgxzma05qyhl"))

(define rust-windows-x86-64-gnullvm-0.48.5
  (crate-source "windows_x86_64_gnullvm" "0.48.5"
                "1k24810wfbgz8k48c2yknqjmiigmql6kk3knmddkv8k8g1v54yqb"))

(define rust-windows-x86-64-gnullvm-0.52.6
  (crate-source "windows_x86_64_gnullvm" "0.52.6"
                "03gda7zjx1qh8k9nnlgb7m3w3s1xkysg55hkd1wjch8pqhyv5m94"))

(define rust-windows-x86-64-msvc-0.48.5
  (crate-source "windows_x86_64_msvc" "0.48.5"
                "0f4mdp895kkjh9zv8dxvn4pc10xr7839lf5pa9l0193i2pkgr57d"))

(define rust-windows-x86-64-msvc-0.52.6
  (crate-source "windows_x86_64_msvc" "0.52.6"
                "1v7rb5cibyzx8vak29pdrk8nx9hycsjs4w0jgms08qk49jl6v7sq"))

(define rust-winnow-0.7.1
  (crate-source "winnow" "0.7.1"
                "0pslmfs69hp44qgca8iys5agkjrs1ng75kv3ni6z8hsgbz3pdqw6"))

(define rust-winsafe-0.0.19
  (crate-source "winsafe" "0.0.19"
                "0169xy9mjma8dys4m8v4x0xhw2gkbhv2v1wsbvcjl9bhnxxd2dfi"))


(define-public cargo-zigbuild
  (package
    (name "cargo-zigbuild")
    (version "0.20.1")
    (source
     (origin
       (method url-fetch)
       (uri (crate-uri "cargo-zigbuild" version))
       (file-name (string-append name "-" version ".tar.gz"))
       (sha256
        (base32 "0n7ns440lz2hhfd5gzfsh2dag7xyingcryv7zfigwzqg8xvlsn59"))))
    (build-system cargo-build-system)
    (inputs (cargo-inputs 'cargo-zigbuild))
    (arguments
     `(#:cargo-inputs
       (;; List ALL the dependencies you defined above here
        ("rust-anstream-0" ,rust-anstream-0.6.18)
        ("rust-anstyle-1" ,rust-anstyle-1.0.10)
        ("rust-anstyle-parse-0" ,rust-anstyle-parse-0.2.6)
        ("rust-anstyle-query-1" ,rust-anstyle-query-1.1.2)
        ("rust-anstyle-wincon-3" ,rust-anstyle-wincon-3.0.7)
        ("rust-anyhow-1" ,rust-anyhow-1.0.98)
        ("rust-autocfg-1" ,rust-autocfg-1.4.0)
        ("rust-bitflags-2" ,rust-bitflags-2.8.0)
        ("rust-camino-1" ,rust-camino-1.1.9)
        ("rust-cargo-config2-0" ,rust-cargo-config2-0.1.32)
        ("rust-cargo-options-0" ,rust-cargo-options-0.7.5)
        ("rust-cargo-platform-0" ,rust-cargo-platform-0.1.6)
        ("rust-cargo-metadata-0" ,rust-cargo-metadata-0.19.0)
        ("rust-cfg-if-1" ,rust-cfg-if-1.0.0)
        ("rust-clap-4" ,rust-clap-4.5.28)
        ("rust-clap-builder-4" ,rust-clap-builder-4.5.27)
        ("rust-clap-derive-4" ,rust-clap-derive-4.5.28)
        ("rust-clap-lex-0" ,rust-clap-lex-0.7.4)
        ("rust-colorchoice-1" ,rust-colorchoice-1.0.3)
        ("rust-crc-3" ,rust-crc-3.2.1)
        ("rust-crc-catalog-2" ,rust-crc-catalog-2.4.0)
        ("rust-dirs-5" ,rust-dirs-5.0.1)
        ("rust-dirs-sys-0" ,rust-dirs-sys-0.4.1)
        ("rust-either-1" ,rust-either-1.13.0)
        ("rust-env-home-0" ,rust-env-home-0.1.0)
        ("rust-equivalent-1" ,rust-equivalent-1.0.1)
        ("rust-errno-0" ,rust-errno-0.3.10)
        ("rust-fat-macho-0" ,rust-fat-macho-0.4.9)
        ("rust-fs-err-3" ,rust-fs-err-3.1.0)
        ("rust-getrandom-0" ,rust-getrandom-0.2.15)
        ("rust-goblin-0" ,rust-goblin-0.9.3)
        ("rust-hashbrown-0" ,rust-hashbrown-0.15.2)
        ("rust-heck-0" ,rust-heck-0.5.0)
        ("rust-indexmap-2" ,rust-indexmap-2.7.1)
        ("rust-is-terminal-polyfill-1" ,rust-is-terminal-polyfill-1.70.1)
        ("rust-itoa-1" ,rust-itoa-1.0.14)
        ("rust-libc-0" ,rust-libc-0.2.169)
        ("rust-libredox-0" ,rust-libredox-0.1.3)
        ("rust-linux-raw-sys-0" ,rust-linux-raw-sys-0.4.15)
        ("rust-log-0" ,rust-log-0.4.25)
        ("rust-memchr-2" ,rust-memchr-2.7.4)
        ("rust-once-cell-1" ,rust-once-cell-1.20.2)
        ("rust-option-ext-0" ,rust-option-ext-0.2.0)
        ("rust-path-slash-0" ,rust-path-slash-0.2.1)
        ("rust-plain-0" ,rust-plain-0.2.3)
        ("rust-proc-macro2-1" ,rust-proc-macro2-1.0.93)
        ("rust-quote-1" ,rust-quote-1.0.38)
        ("rust-redox-users-0" ,rust-redox-users-0.4.6)
        ("rust-rustc-version-0" ,rust-rustc-version-0.4.1)
        ("rust-rustflags-0" ,rust-rustflags-0.1.6)
        ("rust-rustix-0" ,rust-rustix-0.38.44)
        ("rust-ryu-1" ,rust-ryu-1.0.19)
        ("rust-scroll-0" ,rust-scroll-0.12.0)
        ("rust-scroll-derive-0" ,rust-scroll-derive-0.12.0)
        ("rust-semver-1" ,rust-semver-1.0.26)
        ("rust-serde-1" ,rust-serde-1.0.219)
        ("rust-serde-derive-1" ,rust-serde-derive-1.0.219)
        ("rust-serde-json-1" ,rust-serde-json-1.0.140)
        ("rust-serde-spanned-0" ,rust-serde-spanned-0.6.8)
        ("rust-shlex-1" ,rust-shlex-1.3.0)
        ("rust-strsim-0" ,rust-strsim-0.11.1)
        ("rust-syn-2" ,rust-syn-2.0.98)
        ("rust-target-lexicon-0" ,rust-target-lexicon-0.13.1)
        ("rust-terminal-size-0" ,rust-terminal-size-0.4.1)
        ("rust-thiserror-1" ,rust-thiserror-1.0.69)
        ("rust-thiserror-impl-1" ,rust-thiserror-impl-1.0.69)
        ("rust-toml-datetime-0" ,rust-toml-datetime-0.6.8)
        ("rust-toml-edit-0" ,rust-toml-edit-0.22.23)
        ("rust-unicode-ident-1" ,rust-unicode-ident-1.0.16)
        ("rust-utf8parse-0" ,rust-utf8parse-0.2.2)
        ("rust-wasi-0" ,rust-wasi-0.11.0+wasi-snapshot-preview1)
        ("rust-which-7" ,rust-which-7.0.1)
        ("rust-windows-sys-0.48" ,rust-windows-sys-0.48.0)
        ("rust-windows-sys-0" ,rust-windows-sys-0.59.0)
        ("rust-windows-targets-0.48" ,rust-windows-targets-0.48.5)
        ("rust-windows-targets-0" ,rust-windows-targets-0.52.6)
        ("rust-windows-aarch64-gnullvm-0.48" ,rust-windows-aarch64-gnullvm-0.48.5)
        ("rust-windows-aarch64-gnullvm-0" ,rust-windows-aarch64-gnullvm-0.52.6)
        ("rust-windows-aarch64-msvc-0.48" ,rust-windows-aarch64-msvc-0.48.5)
        ("rust-windows-aarch64-msvc-0" ,rust-windows-aarch64-msvc-0.52.6)
        ("rust-windows-i686-gnu-0.48" ,rust-windows-i686-gnu-0.48.5)
        ("rust-windows-i686-gnu-0" ,rust-windows-i686-gnu-0.52.6)
        ("rust-windows-i686-gnullvm-0" ,rust-windows-i686-gnullvm-0.52.6)
        ("rust-windows-i686-msvc-0.48" ,rust-windows-i686-msvc-0.48.5)
        ("rust-windows-i686-msvc-0" ,rust-windows-i686-msvc-0.52.6)
        ("rust-windows-x86-64-gnu-0.48" ,rust-windows-x86-64-gnu-0.48.5)
        ("rust-windows-x86-64-gnu-0" ,rust-windows-x86-64-gnu-0.52.6)
        ("rust-windows-x86-64-gnullvm-0.48" ,rust-windows-x86-64-gnullvm-0.48.5)
        ("rust-windows-x86-64-gnullvm-0" ,rust-windows-x86-64-gnullvm-0.52.6)
        ("rust-windows-x86-64-msvc-0.48" ,rust-windows-x86-64-msvc-0.48.5)
        ("rust-windows-x86-64-msvc-0" ,rust-windows-x86-64-msvc-0.52.6)
        ("rust-winnow-0" ,rust-winnow-0.7.1)
        ("rust-winsafe-0" ,rust-winsafe-0.0.19)
        )))
    (home-page "https://github.com/rust-cross/cargo-zigbuild")
    (synopsis "Compile Cargo project with zig as linker")
    (description
     "This package provides Compile Cargo project with zig as linker.")
    (license license:expat)))

(concatenate-manifests
  (list
    (specifications->manifest
      (list
        "rust"
        "rust:cargo"
        "zig"
        "coreutils-minimal"
        "patchelf"
        "gcc-toolchain"
        "pkg-config"
        "eudev"
        "fontconfig"))
    ;; The GUI's MSRV is 1.80 and the daemon's 1.63. We just use the same rustc version for
    ;; both.
    ;; FIXME: be able to compile against a specified glibc (or musl) instead of having to
    ;; resort to backporting the newer rustc releases here. Also have proper Guix packages
    ;; for the two projects.
    (packages->manifest
      `(,cargo-zigbuild))))
