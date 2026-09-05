# Couverture llvm-cov avec seuil 80 % sur conduit-core, conduit-protocol (M0-96),
# conduit-kmd-core (M1a-01b) et conduit-com (M1a-04).
{ inputs, ... }:
{
  perSystem =
    { pkgs, conduitCrane, ... }:
    let
      inherit (conduitCrane) commonArgs;
      tc = import ./toolchain.nix {
        inherit pkgs;
        rust-overlay = inputs.rust-overlay;
      };
      craneCov = (inputs.crane.mkLib pkgs).overrideToolchain tc.toolchainCov;
      cargoArtifacts = craneCov.buildDepsOnly (
        commonArgs // { cargoExtraArgs = "--workspace --all-features"; }
      );
    in
    {
      checks.coverage = craneCov.cargoLlvmCov (
        commonArgs
        // {
          inherit cargoArtifacts;
          preBuild = "mkdir -p $out";
          cargoLlvmCovExtraArgs = "--all-features -p conduit-core -p conduit-protocol -p conduit-kmd-core -p conduit-com --fail-under-lines 80 --lcov --output-path $out/lcov.info";
        }
      );
    };
}
