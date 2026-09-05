{ inputs, ... }:
{
  perSystem =
    {
      pkgs,
      system,
      lib,
      ...
    }:
    let
      tc = import ./toolchain.nix {
        inherit pkgs;
        rust-overlay = inputs.rust-overlay;
      };
      craneLib = (inputs.crane.mkLib pkgs).overrideToolchain tc.toolchain;
      src = lib.cleanSourceWith {
        src = ../.;
        filter =
          path: type:
          (craneLib.filterCargoSources path type)
          || (
            builtins.match ".*\\.(md|json|toml|ps1|conf)$" path != null
            && builtins.match ".*/target/.*" path == null
          );
      };
      commonArgs = {
        inherit src;
        strictDeps = true;
        pname = "conduit";
        version = "0.1.0";
        # `bindgenHook` : pipewire-sys génère ses bindings avec bindgen (Linux).
        nativeBuildInputs = [
          pkgs.pkg-config
        ]
        ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.rustPlatform.bindgenHook ];
        buildInputs = lib.optionals pkgs.stdenv.hostPlatform.isLinux [
          pkgs.alsa-lib
          pkgs.pipewire
        ];
      };
      # Dépendances vendorisées et compilées une fois, partagées par tous les dérivés.
      cargoArtifacts = craneLib.buildDepsOnly (
        commonArgs // { cargoExtraArgs = "--workspace --all-features"; }
      );
      mkBin =
        name:
        craneLib.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;
            pname = name;
            cargoExtraArgs = "-p ${name}";
            doCheck = false;
            meta.mainProgram = name;
          }
        );
    in
    {
      packages = {
        conduitd = mkBin "conduitd";
        conduitctl = mkBin "conduitctl";
        default = mkBin "conduitd";
      };

      apps = {
        conduitd = {
          type = "app";
          program = "${mkBin "conduitd"}/bin/conduitd";
        };
        conduitctl = {
          type = "app";
          program = "${mkBin "conduitctl"}/bin/conduitctl";
        };
        default = {
          type = "app";
          program = "${mkBin "conduitd"}/bin/conduitd";
        };
      };

      # Exposé aux checks.
      _module.args.conduitCrane = {
        inherit
          craneLib
          commonArgs
          cargoArtifacts
          src
          ;
      };
    };
}
