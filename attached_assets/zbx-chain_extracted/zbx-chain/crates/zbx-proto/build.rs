fn main() {
    let proto_root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../proto");

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .out_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src/generated"))
        .compile_protos(
            &[
                &format!("{proto_root}/consensus.proto"),
                &format!("{proto_root}/da.proto"),
                &format!("{proto_root}/node.proto"),
                &format!("{proto_root}/prover.proto"),
            ],
            &[proto_root],
        )
        .expect("tonic_build proto compilation failed");
}
