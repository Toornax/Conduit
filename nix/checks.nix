{ inputs, ... }:
{
  perSystem =
    {
      pkgs,
      lib,
      conduitCrane,
      ...
    }:
    let
      inherit (conduitCrane) craneLib commonArgs cargoArtifacts;
      tc = import ./toolchain.nix {
        inherit pkgs;
        rust-overlay = inputs.rust-overlay;
      };
      craneCross = (inputs.crane.mkLib pkgs).overrideToolchain tc.toolchainCross;
    in
    {
      checks = {
        fmt = craneLib.cargoFmt { inherit (commonArgs) src; };

        clippy = craneLib.cargoClippy (
          commonArgs
          // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--workspace --all-targets --all-features -- -D warnings";
          }
        );

        tests = craneLib.cargoNextest (
          commonArgs
          // {
            inherit cargoArtifacts;
            cargoNextestExtraArgs = "--workspace --all-features";
            partitions = 1;
            partitionType = "count";
          }
        );

        deny = craneLib.cargoDeny { inherit (commonArgs) src; };

        doc = craneLib.cargoDoc (
          commonArgs
          // {
            inherit cargoArtifacts;
            cargoDocExtraArgs = "--workspace --no-deps --all-features";
            RUSTDOCFLAGS = "-D warnings";
          }
        );

        # La doc du protocole est à jour avec les types (M0-73).
        protocol-docs = craneLib.mkCargoDerivation (
          commonArgs
          // {
            inherit cargoArtifacts;
            pnameSuffix = "-protocol-docs";
            buildPhaseCargoCommand = "cargo run -p conduit-protocol --features schema --example gen-docs -- --check";
            doInstallCargoArtifacts = false;
          }
        );

        # Vérification croisée mingwW64 des crates utilisateur Windows (M0-09).
        cross-windows = craneCross.mkCargoDerivation (
          commonArgs
          // {
            cargoArtifacts = null;
            pnameSuffix = "-cross-windows";
            buildPhaseCargoCommand = "cargo check --workspace --target x86_64-pc-windows-gnu";
            doInstallCargoArtifacts = false;
            nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.pkgsCross.mingwW64.stdenv.cc ];
          }
        );
      };
    };
}
