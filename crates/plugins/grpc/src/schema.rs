use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::anyhow;
use buffa_descriptor::generated::descriptor::method_options::IdempotencyLevel;
use buffa_descriptor::{DescriptorPool, DynamicMessage, MessageIndex};

/// The services of a FileDescriptorSet that rapira serves.
pub struct Schema {
    pool: Arc<DescriptorPool>,
    /// Keyed `package.Service/Method`, the request path without its leading slash.
    methods: HashMap<String, Method>,
    services: Vec<ServiceInfo>,
}

/// A configured service with every method it declares.
#[derive(Debug, PartialEq, Eq)]
pub struct ServiceInfo {
    pub name: String,
    pub methods: Vec<MethodInfo>,
}

/// A method with fully qualified message type names.
#[derive(Debug, PartialEq, Eq)]
pub struct MethodInfo {
    pub name: String,
    pub input_type: String,
    pub output_type: String,
    pub client_streaming: bool,
    pub server_streaming: bool,
}

/// A unary method that rapira routes to PHP.
pub(crate) struct Method {
    pub(crate) input: MessageIndex,
    pub(crate) output: MessageIndex,
    /// The method has `idempotency_level = NO_SIDE_EFFECTS`, so Connect GET can call it.
    pub(crate) idempotent: bool,
}

impl Schema {
    /// Loads the set at `path` and keeps the unary methods of `services` as routes.
    pub fn load(path: &Path, services: &[String]) -> anyhow::Result<Schema> {
        let bytes = std::fs::read(path)
            .map_err(|e| anyhow!("reading grpc.descriptor_set {}: {e}", path.display()))?;
        // The operator supplies the set, so the element memory limit for untrusted input does not apply.
        let opts = buffa::DecodeOptions::new().with_element_memory_limit(usize::MAX);
        let pool = DescriptorPool::decode_with_options(&bytes, &opts).map_err(|e| {
            anyhow!(
                "decoding grpc.descriptor_set {}: {e}; build it with `buf build --as-file-descriptor-set` or `protoc --include_imports`",
                path.display()
            )
        })?;

        let mut methods = HashMap::new();
        let mut listed = Vec::with_capacity(services.len());
        for name in services {
            let service = pool.service_by_name(name).ok_or_else(|| {
                anyhow!(
                    "grpc.services entry `{name}` is not in grpc.descriptor_set {}",
                    path.display()
                )
            })?;
            let full_name = service.full_name();
            if listed.iter().any(|s: &ServiceInfo| s.name == full_name) {
                return Err(anyhow!("grpc.services lists `{full_name}` twice"));
            }
            let mut infos = Vec::with_capacity(service.methods().len());
            for m in service.methods() {
                let route = format!("{}/{}", service.full_name(), m.name());
                if m.is_client_streaming() || m.is_server_streaming() {
                    tracing::warn!(target: "grpc", "{route} streams; rapira answers it with UNIMPLEMENTED");
                } else {
                    let idempotent = m.options().and_then(|o| o.idempotency_level)
                        == Some(IdempotencyLevel::NO_SIDE_EFFECTS);
                    let method = Method {
                        input: m.input(),
                        output: m.output(),
                        idempotent,
                    };
                    methods.insert(route, method);
                }
                infos.push(MethodInfo {
                    name: m.name().to_owned(),
                    input_type: pool.message(m.input()).full_name().to_owned(),
                    output_type: pool.message(m.output()).full_name().to_owned(),
                    client_streaming: m.is_client_streaming(),
                    server_streaming: m.is_server_streaming(),
                });
            }
            listed.push(ServiceInfo {
                name: service.full_name().to_owned(),
                methods: infos,
            });
        }

        Ok(Schema {
            pool: Arc::new(pool),
            methods,
            services: listed,
        })
    }

    /// Every method of the configured services, streaming ones included, in descriptor order.
    pub fn services(&self) -> &[ServiceInfo] {
        &self.services
    }

    pub(crate) fn pool(&self) -> &Arc<DescriptorPool> {
        &self.pool
    }

    /// The route for `path` (`package.Service/Method`). Streaming and unlisted methods have none.
    pub(crate) fn method(&self, path: &str) -> Option<&Method> {
        self.methods.get(path)
    }

    /// Unknown fields are dropped. An unknown enum value name fails the decode.
    pub(crate) fn json_to_proto(&self, m: &Method, json: &[u8]) -> anyhow::Result<Vec<u8>> {
        let json = std::str::from_utf8(json)?;
        let msg =
            DynamicMessage::from_json_ignoring_unknown(Arc::clone(&self.pool), m.input, json)?;
        Ok(msg.encode_to_vec())
    }

    pub(crate) fn proto_to_json(&self, m: &Method, bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
        // The application produces the reply, so the element memory limit for untrusted input does not apply.
        let opts = buffa::DecodeOptions::new().with_element_memory_limit(usize::MAX);
        let msg =
            DynamicMessage::decode_with_options(Arc::clone(&self.pool), m.output, bytes, &opts)?;
        Ok(msg.to_json()?.into_bytes())
    }
}
