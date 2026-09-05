# Toolchain Rust lue depuis rust-toolchain.toml (source de vérité, SPEC §5.11).
{ pkgs, rust-overlay }:
let
  pkgsWithOverlay = pkgs.extend (import rust-overlay);
  toolchain = pkgsWithOverlay.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml;
  # Cible de vérification croisée Windows (M0-09).
  toolchainCross = toolchain.override {
    targets = [ "x86_64-pc-windows-gnu" ];
  };
  # Toolchain avec llvm-tools pour la couverture (M0-96).
  toolchainCov = toolchain.override {
    extensions = [ "llvm-tools-preview" ];
  };
  nightly = pkgsWithOverlay.rust-bin.nightly.latest.minimal.override {
    extensions = [ "miri" "rust-src" ];
  };
in
{
  inherit pkgsWithOverlay toolchain toolchainCross toolchainCov nightly;
}
