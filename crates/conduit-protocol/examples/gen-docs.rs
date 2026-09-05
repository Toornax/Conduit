//! Génère `docs/protocol.md` et `docs/protocol.schema.json`.
//!
//! `cargo run -p conduit-protocol --features schema --example gen-docs`
//! Avec `--check`, échoue si les fichiers ne sont pas à jour (CI).

fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let md_path = root.join("docs/protocol.md");
    let schema_path = root.join("docs/protocol.schema.json");
    let md = conduit_protocol::schema::protocol_markdown();
    let schema = conduit_protocol::schema::message_schema() + "\n";
    let check = std::env::args().any(|a| a == "--check");
    if check {
        let ok = std::fs::read_to_string(&md_path)
            .map(|s| s == md)
            .unwrap_or(false)
            && std::fs::read_to_string(&schema_path)
                .map(|s| s == schema)
                .unwrap_or(false);
        if !ok {
            eprintln!("docs/protocol.md ou docs/protocol.schema.json n'est pas à jour : lancez gen-docs sans --check");
            std::process::exit(1);
        }
        println!("docs du protocole à jour");
    } else {
        std::fs::write(&md_path, md).expect("écriture protocol.md");
        std::fs::write(&schema_path, schema).expect("écriture protocol.schema.json");
        println!("écrit {} et {}", md_path.display(), schema_path.display());
    }
}
