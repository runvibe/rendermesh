use crate::dto::echo::{EchoRequestInput, EchoResponse};

pub fn echo(input: EchoRequestInput) -> EchoResponse {
    EchoResponse {
        headers: input.headers,
        path: input.path,
        method: input.method,
        body: input.body,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::echo;
    use crate::dto::echo::EchoRequestInput;

    #[test]
    fn echo_mirrors_request_input() {
        let mut headers = BTreeMap::new();
        headers.insert("x-test".to_string(), vec!["value".to_string()]);

        let response = echo(EchoRequestInput {
            headers: headers.clone(),
            path: "/echo".to_string(),
            method: "POST".to_string(),
            body: Some("payload".to_string()),
        });

        assert_eq!(response.headers, headers);
        assert_eq!(response.path, "/echo");
        assert_eq!(response.method, "POST");
        assert_eq!(response.body.as_deref(), Some("payload"));
    }
}
