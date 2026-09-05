# Couverture llvm-cov avec seuil 80 % sur conduit-core et conduit-protocol (M0-96).
{ ... }:
{
  perSystem = { pkgs, conduitCrane, ... }:
    let
      inherit (conduitCrane) craneLib commonArgs cargoArtifacts;
    in
    {
      checks.coverage = craneLib.cargoLlvmCov (commonArgs // {
        inherit cargoArtifacts;
        cargoLlvmCovExtraArgs = "--all-features -p conduit-core -p conduit-protocol --fail-under-lines 80 --summary-only";
      });
    };
}
