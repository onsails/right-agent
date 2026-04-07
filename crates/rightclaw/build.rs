fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_dir = "../../proto/openshell";

    tonic_prost_build::configure()
        .build_server(false)
        .compile_protos(
            &[
                format!("{proto_dir}/sandbox.proto"),
                format!("{proto_dir}/datamodel.proto"),
                format!("{proto_dir}/openshell.proto"),
            ],
            &[proto_dir.to_string()],
        )?;

    Ok(())
}
