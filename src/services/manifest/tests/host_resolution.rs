use super::*;
    #[test]
    fn exact_host_wins_over_wildcard() {
        let manifest = parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  admin:
    type: s3
    bucket: admin
    endpoint_env: ADMIN_ENDPOINT
    region_env: ADMIN_REGION
    access_key_id_env: ADMIN_KEY
    secret_access_key_env: ADMIN_SECRET
  web:
    type: s3
    bucket: web
    endpoint_env: WEB_ENDPOINT
    region_env: WEB_REGION
    access_key_id_env: WEB_KEY
    secret_access_key_env: WEB_SECRET
hosts:
  admin.megaloja.com.br:
    origin: admin
  "*.megaloja.com.br":
    origin: web
"#,
        )
        .expect("manifest parses");

        let resolver = HostResolver::new(&manifest).expect("resolver builds");
        let resolved = resolver
            .resolve("ADMIN.megaloja.com.br:443")
            .expect("host resolves");

        assert_eq!(resolved.origin_id, "admin");
        assert_eq!(resolved.matched_host, "admin.megaloja.com.br");
    }


    #[test]
    fn most_specific_wildcard_wins() {
        let manifest = parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  broad:
    type: s3
    bucket: broad
    endpoint_env: BROAD_ENDPOINT
    region_env: BROAD_REGION
    access_key_id_env: BROAD_KEY
    secret_access_key_env: BROAD_SECRET
  narrow:
    type: s3
    bucket: narrow
    endpoint_env: NARROW_ENDPOINT
    region_env: NARROW_REGION
    access_key_id_env: NARROW_KEY
    secret_access_key_env: NARROW_SECRET
hosts:
  "*.megaloja.com.br":
    origin: broad
  "*.admin.megaloja.com.br":
    origin: narrow
"#,
        )
        .expect("manifest parses");

        let resolver = HostResolver::new(&manifest).expect("resolver builds");
        let resolved = resolver
            .resolve("x.admin.megaloja.com.br")
            .expect("host resolves");

        assert_eq!(resolved.origin_id, "narrow");
    }


    #[test]
    fn unknown_host_is_none() {
        let manifest = parse_manifest_yaml(sample_manifest()).expect("manifest parses");
        let resolver = HostResolver::new(&manifest).expect("resolver builds");

        assert!(resolver.resolve("unknown.test").is_none());
    }


    #[test]
    fn global_wildcard_resolves_unknown_valid_host() {
        let manifest = parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  fallback:
    type: s3
    bucket: fallback
    endpoint_env: FALLBACK_ENDPOINT
    region_env: FALLBACK_REGION
hosts:
  "*":
    origin: fallback
"#,
        )
        .expect("manifest parses");

        let resolver = HostResolver::new(&manifest).expect("resolver builds");
        let resolved = resolver
            .resolve("UNRELATED.test:8443")
            .expect("global wildcard resolves");

        assert_eq!(resolved.normalized_host, "unrelated.test");
        assert_eq!(resolved.matched_host, "*");
        assert_eq!(resolved.origin_id, "fallback");
    }


    #[test]
    fn specific_hosts_take_priority_over_global_wildcard() {
        let manifest = parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  exact:
    type: local
    path: ./exact
  domain:
    type: local
    path: ./domain
  fallback:
    type: local
    path: ./fallback
hosts:
  admin.example.com:
    origin: exact
  "*.example.com":
    origin: domain
  "*":
    origin: fallback
"#,
        )
        .expect("manifest parses");
        let resolver = HostResolver::new(&manifest).expect("resolver builds");

        assert_eq!(
            resolver
                .resolve("admin.example.com")
                .expect("exact host resolves")
                .origin_id,
            "exact"
        );
        assert_eq!(
            resolver
                .resolve("shop.example.com")
                .expect("domain wildcard resolves")
                .origin_id,
            "domain"
        );
        assert_eq!(
            resolver
                .resolve("unrelated.test")
                .expect("global wildcard resolves")
                .origin_id,
            "fallback"
        );
    }


    #[test]
    fn global_wildcard_does_not_resolve_invalid_host() {
        let manifest = parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  fallback:
    type: local
    path: ./fallback
hosts:
  "*":
    origin: fallback
"#,
        )
        .expect("manifest parses");
        let resolver = HostResolver::new(&manifest).expect("resolver builds");

        assert!(resolver.resolve("").is_none());
        assert!(resolver.resolve("invalid host").is_none());
        assert!(resolver.resolve("example.com:not-a-port").is_none());
    }


    #[test]
    fn rejects_duplicate_normalized_global_wildcards() {
        let manifest = parse_manifest_yaml(
            r#"
version: 1
runtime:
  local_store_dir: ./var/rendermesh/origins
  sync_interval_seconds: 60
origins:
  fallback:
    type: local
    path: ./fallback
hosts:
  "*":
    origin: fallback
  " * ":
    origin: fallback
"#,
        )
        .expect("manifest parses");

        let error = HostResolver::new(&manifest).expect_err("duplicate wildcard is rejected");

        assert!(error.to_string().contains("duplicate global wildcard host"));
    }
