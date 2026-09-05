{ inputs, ... }:
{
  perSystem =
    {
      pkgs,
      system,
      config,
      ...
    }:
    let
      tc = import ./toolchain.nix {
        inherit pkgs;
        rust-overlay = inputs.rust-overlay;
      };
      linuxOnly = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux (
        with pkgs;
        [
          pipewire.dev
          # Banc de test headless (M3) : le démon `pipewire` et ses outils, plus
          # `wireplumber`, sans lequel aucun flux client n'est relié ni cadencé.
          pipewire
          wireplumber
          alsa-lib.dev
          wayland
          libxkbcommon
          vulkan-loader
          libGL
          xorg.libX11
          xorg.libXcursor
          xorg.libXi
          xorg.libXrandr
        ]
      );
      darwinOnly = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin (
        with pkgs;
        [
          apple-sdk
        ]
      );
    in
    {
      devShells.default = pkgs.mkShell {
        name = "conduit";
        packages =
          with pkgs;
          [
            tc.toolchainCross
            cargo-nextest
            cargo-deny
            cargo-audit
            cargo-llvm-cov
            clang
            pkg-config
            nixfmt-rfc-style
          ]
          ++ linuxOnly
          ++ darwinOnly;
        LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
        inputsFrom = [ config.pre-commit.devShell ];
        shellHook = ''
          ${config.pre-commit.installationScript}
          echo "conduit devshell — $(cargo --version) — nix flake check pour valider"
        '';
      };

      # Shell nightly séparé : Miri et cargo-fuzz (SPEC §5.11).
      devShells.nightly = pkgs.mkShell {
        name = "conduit-nightly";
        packages = [
          tc.nightly
          pkgs.cargo-fuzz
        ];
      };
    };
}
