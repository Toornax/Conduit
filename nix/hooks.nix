# Hooks de pré-commit via git-hooks.nix (M0-06) : rustfmt, nixfmt, cargo-deny,
# format des messages de commit (Conventional Commits, scopes de ROADMAP).
{ inputs, ... }:
{
  imports = [ inputs.git-hooks.flakeModule ];

  perSystem = { pkgs, config, ... }:
    let
      types = "feat|fix|test|docs|chore|ci|refactor|perf|bench";
      scopes = "core|engine|backend|null|wasapi|pipewire|coreaudio|protocol|daemon|cli|gui|driver|portcls|helper|hal|packaging|nix|testing";
      commitMsg = pkgs.writeShellScript "check-commit-msg" ''
        first=$(head -n1 "$1")
        if [[ "$first" =~ ^(Merge|Revert|fixup!|squash!) ]]; then exit 0; fi
        if ! [[ "$first" =~ ^(${types})(\((${scopes})(,\ ?(${scopes}))*\))?!?:\ .+ ]]; then
          echo "message de commit refusé : « $first »" >&2
          echo "attendu : type(scope): description à l'impératif" >&2
          echo "types : ${types}" >&2
          echo "scopes : ${scopes}" >&2
          exit 1
        fi
      '';
    in
    {
      pre-commit.settings.hooks = {
        rustfmt = {
          enable = true;
          packageOverrides.cargo = pkgs.cargo;
          packageOverrides.rustfmt = pkgs.rustfmt;
        };
        nixfmt-rfc-style.enable = true;
        cargo-deny = {
          enable = true;
          name = "cargo-deny";
          entry = "${pkgs.cargo-deny}/bin/cargo-deny check";
          pass_filenames = false;
          files = "(Cargo\\.(toml|lock)|deny\\.toml)$";
        };
        commit-message = {
          enable = true;
          name = "format du message de commit";
          entry = "${commitMsg}";
          stages = [ "commit-msg" ];
          pass_filenames = true;
        };
      };
    };
}
