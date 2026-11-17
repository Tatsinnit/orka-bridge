use bridge_guest::{
    model::{ModelCommand, ModelLifecycle, Outcome, Runner},
    service,
};
use bridge_runtime_protocol::*;
#[test]
fn local_socket_cannot_read_the_workspace_catalog_or_reach_orka() {
    for action in [
        EngineActionWire::WorkspaceCatalog {},
        serde_json::from_value(serde_json::json!({"type":"cleanupRunAgent","uid":"recorded-uid"}))
            .unwrap(),
        EngineActionWire::OrkaChat {
            request: ChatRequest {
                method: ChatMethod::Delete,
                path: "tasks/bridge-chat-task-0123456789abcdef0123456789abcdef".into(),
                after: None,
                body: None,
            },
        },
        EngineActionWire::Orka {
            request: OrkaRequest {
                path: "agents".into(),
                after: None,
                body: None,
            },
        },
    ] {
        assert!(bridge_guest::engine::argv(&action).is_err());
        let r = RequestBody::Execute {
            operation: Uuid::new_v4(),
            action: action.clone(),
            snapshot: None,
        };
        assert!(!service::authoring_allowed(false, &r));
        assert!(service::authoring_allowed(true, &r));
        assert!(!service::authoring_allowed(
            false,
            &RequestBody::Engine(action)
        ));
    }
    assert!(service::authoring_allowed(
        false,
        &RequestBody::Execute {
            operation: Uuid::new_v4(),
            action: EngineActionWire::Version {},
            snapshot: None
        }
    ));
}
struct Fake;
impl Runner for Fake {
    fn execute(&mut self, _: &ModelCommand) -> Outcome {
        Outcome {
            connection: Some(connection()),
            success: true,
            diagnostic: None,
        }
    }
}
fn connection() -> ModelConnection {
    ModelConnection {
        id: Uuid::from_u128(2),
        selection: ModelSelection {
            vendor: None,
            provider: "copilot".into(),
            model: "m".into(),
        },
        generation: Uuid::from_u128(1),
    }
}
#[test]
fn only_the_acknowledged_ready_connection_can_supply_catalog_provider() {
    let mut lifecycle = ModelLifecycle::new(Fake);
    assert!(lifecycle.ready_connection().is_none());
    let credential = CredentialState {
        profile: CREDENTIAL_PROFILE,
        generation: Some(Uuid::from_u128(1)),
        installed: true,
    };
    lifecycle.handle(
        credential.clone(),
        &RequestBody::ModelConnect {
            generation: Uuid::from_u128(1),
            selection: connection().selection,
        },
    );
    assert_eq!(lifecycle.ready_connection(), Some(&connection()));
    lifecycle.handle(
        credential,
        &RequestBody::ModelDisconnect {
            connection: Uuid::from_u128(2),
        },
    );
    assert!(lifecycle.ready_connection().is_none());
}
