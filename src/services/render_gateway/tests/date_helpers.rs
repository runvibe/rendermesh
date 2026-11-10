use super::*;

#[tokio::test]
async fn edge_renders_current_time_with_empty_params_but_not_without_params() {
    let temp = tempfile::tempdir().expect("tempdir");
    let template = "<time>{{date_time}}</time>";
    write_object(temp.path(), "index.html", template, None).await;
    let template_store = TemplateStore::default();
    template_store
        .load_origin_templates(
            "web",
            &LocalMirrorRepository::new(temp.path().join("origins")),
        )
        .await
        .expect("templates load");

    let render_server = edge_server(200, serde_json::json!({"params": {}})).await;
    let render_service = test_gateway_with_edge_url_and_templates(
        temp.path().join("origins"),
        &render_server.uri(),
        template_store.clone(),
    );
    let rendered = render_service
        .handle(test_request(Method::GET, "/"))
        .await
        .expect("rendered response");
    assert_eq!(rendered.status, StatusCode::OK);
    let body = std::str::from_utf8(&rendered.body).expect("UTF-8 body");
    let time = body
        .strip_prefix("<time>")
        .and_then(|body| body.strip_suffix("</time>"))
        .expect("time element");
    chrono::NaiveTime::parse_from_str(time, "%H:%M:%S").expect("UTC time");

    let static_server = edge_server(200, serde_json::json!({})).await;
    let static_service = test_gateway_with_edge_url_and_templates(
        temp.path().join("origins"),
        &static_server.uri(),
        template_store,
    );
    let unchanged = static_service
        .handle(test_request(Method::GET, "/"))
        .await
        .expect("static response");
    assert_eq!(unchanged.status, StatusCode::OK);
    assert_eq!(unchanged.body, bytes::Bytes::from(template));
}

#[tokio::test]
async fn edge_date_format_invalid_input_returns_bad_gateway() {
    let server = edge_server(200, serde_json::json!({"params": {"date": "invalid"}})).await;
    let temp = tempfile::tempdir().expect("tempdir");
    write_object(
        temp.path(),
        "index.html",
        "{{date_format date \"%F\"}}",
        None,
    )
    .await;
    let template_store = TemplateStore::default();
    template_store
        .load_origin_templates(
            "web",
            &LocalMirrorRepository::new(temp.path().join("origins")),
        )
        .await
        .expect("templates load");
    let error = template_store
        .render(
            "web",
            "/index.html",
            &serde_json::json!({"date": "invalid"}),
        )
        .expect_err("invalid date must fail");
    assert!(matches!(
        error,
        crate::services::template_store::TemplateStoreError::Render(error)
            if !matches!(error.reason(), handlebars::RenderErrorReason::HelperNotFound(_))
    ));
    let service = test_gateway_with_edge_url_and_templates(
        temp.path().join("origins"),
        &server.uri(),
        template_store,
    );

    let response = service
        .handle(test_request(Method::GET, "/"))
        .await
        .expect("response");
    assert_eq!(response.status, StatusCode::BAD_GATEWAY);
}
