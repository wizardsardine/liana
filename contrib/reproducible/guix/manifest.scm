(use-modules
  (gnu packages llvm)
  (gnu packages rust)
  (guix packages)
  (guix utils))

;; Stolen from Bitcoin Core. Some patches are needed to bootstrap a newer rustc.
(define-syntax-rule (search-our-patches file-name ...)
  "Return the list of absolute file names corresponding to each
FILE-NAME found in ./patches relative to the current file."
  (parameterize
      ((%patch-path (list (string-append (dirname (current-filename)) "/patches"))))
    (list (search-patch file-name) ...)))

;; What follows are the declaration of more recent rustc versions from the current Guix master
;; (https://git.savannah.gnu.org/cgit/guix.git/tree/gnu/packages/rust.scm), which we can't use
;; because it would require an unreasonable glibc version.

(define rust-bootstrapped-package
  (@@ (gnu packages rust) rust-bootstrapped-package))

(define %cargo-reference-hash
  (@@ (gnu packages rust) %cargo-reference-hash))

(define rust-1.65
  (@@ (gnu packages rust) rust-1.65))

(define rust-1.66
  (rust-bootstrapped-package
   rust-1.65 "1.66.1" "1fjr94gsicsxd2ypz4zm8aad1zdbiccr7qjfbmq8f8f7jhx96g2v"))

(define rust-1.67
  (let ((base-rust
          (rust-bootstrapped-package
            rust-1.66 "1.67.1" "0vpzv6rm3w1wbni17ryvcw83k5klhghklylfdza3nnp8blz3sj26")))
    (package
      (inherit base-rust)
      (source
       (origin
         (inherit (package-source base-rust))
         (snippet
          '(begin
             (for-each delete-file-recursively
                       '("src/llvm-project"
                         "vendor/openssl-src/openssl"
                         "vendor/tikv-jemalloc-sys/jemalloc"))
             ;; Adjust rustix to always build with cc.
             (substitute* '("Cargo.lock"
                            "src/bootstrap/Cargo.lock")
               (("\"errno\",") "\"cc\",\n \"errno\","))
             ;; Add a dependency on the the 'cc' feature of rustix.
             (substitute* "vendor/fd-lock/Cargo.toml"
               (("\"fs\"") "\"fs\", \"cc\""))
             (substitute* "vendor/is-terminal/Cargo.toml"
               (("\"termios\"") "\"termios\", \"cc\""))
             ;; Remove vendored dynamically linked libraries.
             ;; find . -not -type d -executable -exec file {} \+ | grep ELF
             (delete-file "vendor/vte/vim10m_match")
             (delete-file "vendor/vte/vim10m_table")
             ;; Also remove the bundled (mostly Windows) libraries.
             (for-each delete-file
                       (find-files "vendor" "\\.(a|dll|exe|lib)$"))))))
      (inputs (modify-inputs (package-inputs base-rust)
                             (replace "llvm" llvm-15))))))

(define rust-1.68
  (rust-bootstrapped-package
   rust-1.67 "1.68.2" "15ifyd5jj8rd979dkakp887hgmhndr68pqaqvd2hqkfdywirqcwk"))

(define rust-1.69
  (let ((base-rust
          (rust-bootstrapped-package
            rust-1.68 "1.69.0"
            "03zn7kx5bi5mdfsqfccj4h8gd6abm7spj0kjsfxwlv5dcwc9f1gv")))
    (package
      (inherit base-rust)
      (source
        (origin
          (inherit (package-source base-rust))
          (snippet
           '(begin
              (for-each delete-file-recursively
                        '("src/llvm-project"
                          "vendor/openssl-src/openssl"
                          "vendor/tikv-jemalloc-sys/jemalloc"))
              ;; Adjust rustix to always build with cc.
              (substitute* '("Cargo.lock"
                             "src/bootstrap/Cargo.lock")
                (("\"errno\",") "\"cc\",\n \"errno\","))
              ;; Add a dependency on the the 'cc' feature of rustix.
              (substitute* "vendor/fd-lock/Cargo.toml"
                (("\"fs\"") "\"fs\", \"cc\""))
              (substitute* "vendor/is-terminal/Cargo.toml"
                (("\"termios\"") "\"termios\", \"cc\""))
              ;; Also remove the bundled (mostly Windows) libraries.
              (for-each delete-file
                        (find-files "vendor" "\\.(a|dll|exe|lib)$")))))))))

(define rust-1.70
  (let ((base-rust
         (rust-bootstrapped-package
          rust-1.69 "1.70.0"
                      "0z6j7d0ni0rmfznv0w3mrf882m11kyh51g2bxkj40l3s1c0axgxj")))
   (package
     (inherit base-rust)
     (source
      (origin
        (inherit (package-source base-rust))
        (snippet
         '(begin
            (for-each delete-file-recursively
                      '("src/llvm-project"
                        "vendor/openssl-src/openssl"
                        "vendor/tikv-jemalloc-sys/jemalloc"))
             ;; Adjust rustix to always build with cc.
             (substitute* "Cargo.lock"
               (("\"errno\",") "\"cc\",\n \"errno\","))
            ;; Add a dependency on the the 'cc' feature of rustix.
            (substitute* '("vendor/is-terminal/Cargo.toml"
                           "vendor/is-terminal-0.4.4/Cargo.toml")
              (("\"termios\"") "\"termios\", \"cc\""))
            ;; Also remove the bundled (mostly Windows) libraries.
            (for-each delete-file
                      (find-files "vendor" "\\.(a|dll|exe|lib)$"))))
        ;; Rust 1.70 adds the rustix library which depends on the vendored
        ;; fd-lock crate.  The fd-lock crate uses Outline assembly which expects
        ;; a precompiled static library.  Enabling the "cc" feature tells the
        ;; build.rs script to compile the assembly files instead of searching
        ;; for a precompiled library.
        (patches (search-our-patches "rust-1.70-fix-rustix-build.patch")))))))

(define rust-1.71
  (let ((base-rust
          (rust-bootstrapped-package
           rust-1.70 "1.71.1" "0bj79syjap1kgpg9pc0r4jxc0zkxwm6phjf3digsfafms580vabg")))
    (package
      (inherit base-rust)
      (source
       (origin
         (inherit (package-source base-rust))
         (snippet
          '(begin
             (for-each delete-file-recursively
                       '("src/llvm-project"
                         "vendor/openssl-src/openssl"
                         "vendor/tikv-jemalloc-sys/jemalloc"))
             ;; Adjust rustix to always build with cc.
             (substitute* '("Cargo.lock"
                            "src/tools/cargo/Cargo.lock")
               (("\"errno\",") "\"cc\",\n \"errno\","))
             ;; Add a dependency on the the 'cc' feature of rustix.
             (substitute* '("vendor/is-terminal/Cargo.toml"
                            "vendor/is-terminal-0.4.6/Cargo.toml")
               (("\"termios\"") "\"termios\", \"cc\""))
             ;; Also remove the bundled (mostly Windows) libraries.
             (for-each delete-file
                       (find-files "vendor" "\\.(a|dll|exe|lib)$"))))))
      (arguments
       (substitute-keyword-arguments (package-arguments base-rust)
         ((#:phases phases)
          `(modify-phases ,phases
             (replace 'patch-cargo-checksums
               (lambda _
                 (substitute* (cons* "Cargo.lock"
                                     "src/bootstrap/Cargo.lock"
                                     (find-files "src/tools" "Cargo.lock"))
                   (("(checksum = )\".*\"" all name)
                    (string-append name "\"" ,%cargo-reference-hash "\"")))
                 (generate-all-checksums "vendor"))))))))))

;; END of the newer rustc versions copied over from the current Guix master.

(concatenate-manifests
  (list
    (specifications->manifest
      (list
        "rust:cargo"
        "coreutils-minimal"
        "patchelf"
        "gcc-toolchain@10.3.0"
        "pkg-config"
        "eudev"
        "fontconfig"))
    ;; The GUI's MSRV is 1.70 and the daemon's 1.63. We just use the same rustc version for
    ;; both.
    ;; FIXME: be able to compile against a specified glibc (or musl) instead of having to
    ;; resort to backporting the newer rustc releases here. Also have proper Guix packages
    ;; for the two projects.
    (packages->manifest
      `(,rust-1.71))))
