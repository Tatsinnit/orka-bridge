use crate::engine::types::EngineAction;
use bridge_runtime_protocol::*;
/// Project the delivered engine contract, never arbitrary argv or environment.
/// No engine execution or renderer availability is enabled by this projection.
pub fn engine_request(nonce: Uuid, id: u64, action: EngineAction) -> WireRequest {
    let action = match action {
        EngineAction::Version {} => EngineActionWire::Version {},
        EngineAction::ListAgents { context, namespace } => {
            EngineActionWire::ListAgents { context, namespace }
        }
    };
    WireRequest {
        version: VERSION,
        boot_nonce: nonce,
        request_id: id,
        body: RequestBody::Engine(action),
    }
}
