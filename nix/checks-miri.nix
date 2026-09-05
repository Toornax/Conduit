# Check Miri sur conduit-core avec la toolchain nightly séparée (M0-44).
{ inputs, ... }:
{
  perSystem = { pkgs, conduitCrane, ... }:
    let
      tc = import ./toolchain.nix { inherit pkgs; rust-overlay = inputs.rust-overlay; };
      craneNightly = (inputs.crane.mkLib pkgs).overrideToolchain tc.nightly;
    in
    {
      checks.miri = craneNightly.mkCargoDerivation (conduitCrane.commonArgs // {
        cargoArtifacts = null;
        pnameSuffix = "-miri";
        MIRIFLAGS = "-Zmiri-disable-isolation";
        buildPhaseCargoCommand = ''
          cargo miri setup
          cargo miri test -p conduit-core --lib -- \
            ring:: graph:: gain:: executor:: slot:: buffer:: types:: param:: \
            --skip block_ops_preserve_order --skip multithreaded_stream \
            --skip concurrent_publish --skip nodes_are_never_dropped
        '';
        doInstallCargoArtifacts = false;
      });
    };
}
