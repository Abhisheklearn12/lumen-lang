{
  description = "Lumen - a small statically-typed language with a full compiler pipeline and bytecode VM";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
    }:
    let
      inherit (nixpkgs) lib;

      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      # Only the files the build actually reads. Touching README.md or docs/
      # must not invalidate a cached build.
      src = lib.fileset.toSource {
        root = ./.;
        fileset = lib.fileset.unions [
          ./Cargo.toml
          ./Cargo.lock
          ./rust-toolchain.toml
          ./src
          ./tests
          ./benches
          ./examples
        ];
      };

      cargoToml = lib.importTOML ./Cargo.toml;
      rustToolchainToml = lib.importTOML ./rust-toolchain.toml;

      each = lib.genAttrs systems;

      # One evaluation of nixpkgs (and one toolchain) per system, shared by
      # every output below.
      perSystem = each (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };

          # rust-toolchain.toml is the single source of truth for the compiler
          # version, here and under rustup. There is no second place to bump.
          toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

          # The same toolchain plus the two components an editor wants and a
          # build does not. The version and the required components still come
          # from rust-toolchain.toml, so this cannot drift either.
          devToolchain = toolchain.override {
            extensions = rustToolchainToml.toolchain.components ++ [
              "rust-src"
              "rust-analyzer"
            ];
          };

          rustPlatform = pkgs.makeRustPlatform {
            cargo = toolchain;
            rustc = toolchain;
          };

          lumen = rustPlatform.buildRustPackage {
            pname = cargoToml.package.name;
            inherit (cargoToml.package) version;
            inherit src;

            cargoLock.lockFile = ./Cargo.lock;

            # `cargo test` shells out to a C compiler for the C-backend
            # differential tests; stdenv's cc satisfies find_cc().
            cargoTestFlags = [ "--locked" ];

            meta = {
              inherit (cargoToml.package) description;
              homepage = cargoToml.package.repository;
              license = lib.licenses.mit;
              mainProgram = "lumenc";
            };
          };

          # Lints reuse the package derivation so they inherit the vendored
          # registry; the build and install phases are replaced and the test
          # suite is skipped, since that is the `build` check's job.
          lintOnly =
            name: command:
            lumen.overrideAttrs (old: {
              pname = "${old.pname}-${name}";
              buildPhase = ''
                runHook preBuild
                ${command}
                runHook postBuild
              '';
              doCheck = false;
              installPhase = ''
                runHook preInstall
                touch $out
                runHook postInstall
              '';
            });
        in
        {
          inherit
            pkgs
            devToolchain
            lumen
            lintOnly
            ;
        }
      );
    in
    {
      packages = each (
        system:
        let
          inherit (perSystem.${system}) lumen;
        in
        {
          inherit lumen;
          default = lumen;
        }
      );

      devShells = each (
        system:
        let
          inherit (perSystem.${system}) pkgs devToolchain;
        in
        {
          default = pkgs.mkShell {
            packages = [
              devToolchain
              pkgs.cargo-insta # `cargo insta review` for the snapshot tests
            ];

            # rust-analyzer resolves std sources from here.
            RUST_SRC_PATH = "${devToolchain}/lib/rustlib/src/rust/library";

            shellHook = ''
              echo "lumen dev shell - $(rustc --version), $(cc --version | head -n1)"
            '';
          };
        }
      );

      # A subset of .github/workflows/ci.yml: clippy and rustfmt as CI runs
      # them, but the test suite only in release. CI additionally runs it in
      # debug, where overflow checks are on.
      checks = each (
        system:
        let
          inherit (perSystem.${system}) lumen lintOnly;
        in
        {
          build = lumen;
          clippy = lintOnly "clippy" "cargo clippy --all-targets --all-features --locked -- -D warnings";
          fmt = lintOnly "fmt" "cargo fmt --check";
        }
      );

      formatter = each (system: perSystem.${system}.pkgs.nixfmt);
    };
}
