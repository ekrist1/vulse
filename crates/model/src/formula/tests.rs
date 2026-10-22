use super::eval::{Ctx, eval};
use super::functions::function;
use super::*;
use serde_json::json;
use std::sync::Arc;
fn run(formula: &str) -> Result<Value, String> {
    let expr = parse(formula)?;
    let ctx = Ctx {
        all: &HashMap::new(),
        body: &json!({"nums":[1,2,3,4],"blanks":[null,"",2],"words":["a","b","a"],"words2":["a","a","b","A"]}),
        lookups: &Lookups::default(),
    };
    eval(&expr, &ctx, &mut HashMap::new())
}
#[test]
fn every_catalog_function_has_a_behavior_test() {
    let cases = [
        ("SUM([nums])", json!(10)),
        ("AVERAGE([nums])", json!(2.5)),
        ("MIN([nums])", json!(1)),
        ("MAX([nums])", json!(4)),
        ("COUNT([blanks])", json!(1)),
        ("COUNTA([blanks])", json!(2)),
        ("COUNTBLANK([blanks])", json!(2)),
        ("PRODUCT([nums])", json!(24)),
        ("MEDIAN([nums])", json!(2.5)),
        ("ABS(-4)", json!(4)),
        ("ROUND(-1.25,1)", json!(-1.3)),
        ("ROUNDUP(-1.21,1)", json!(-1.3)),
        ("ROUNDDOWN(-1.29,1)", json!(-1.2)),
        ("INT(-1.2)", json!(-2)),
        ("MOD(-3,2)", json!(1)),
        ("POWER(2,3)", json!(8)),
        ("SQRT(9)", json!(3)),
        ("CEILING(3.1,2)", json!(4)),
        ("FLOOR(3.1,2)", json!(2)),
        ("IF(TRUE,7,1/0)", json!(7)),
        ("IFERROR(1/0,9)", json!(9)),
        ("AND(TRUE,1)", json!(true)),
        ("OR(FALSE,1)", json!(true)),
        ("NOT(0)", json!(true)),
        ("CONCAT(\"a\",\"b\")", json!("ab")),
        ("CONCATENATE(\"a\",\"b\")", json!("ab")),
        ("TEXTJOIN(\",\",TRUE,[blanks])", json!("2")),
        ("LEFT(\"føø\",2)", json!("fø")),
        ("RIGHT(\"føø\",2)", json!("øø")),
        ("MID(\"hello\",2,3)", json!("ell")),
        ("LEN(\"føø\")", json!(3)),
        ("LOWER(\"HELLO\")", json!("hello")),
        ("UPPER(\"hello\")", json!("HELLO")),
        ("PROPER(\"hello WORLD\")", json!("Hello World")),
        ("TRIM(\"  a  b  \")", json!("a b")),
        ("SUBSTITUTE(\"a-a-a\",\"a\",\"b\",2)", json!("a-b-a")),
        ("REPLACE(\"hello\",2,3,\"i\")", json!("hio")),
        ("FIND(\"ø\",\"føø\",3)", json!(3)),
        ("SEARCH(\"LO\",\"hello\")", json!(4)),
        ("EXACT(\"A\",\"a\")", json!(false)),
        ("VALUE(\"2.5\")", json!(2.5)),
        ("DATE(2024,2,29)", json!(45351)),
        ("YEAR(DATE(2024,2,29))", json!(2024)),
        ("MONTH(\"2024-02-29\")", json!(2)),
        ("DAY(\"2024-02-29\")", json!(29)),
        ("SUMIF([nums],\">2\")", json!(7)),
        ("COUNTIF([words],\"a\")", json!(2)),
        ("AVERAGEIF([nums],\">2\")", json!(3.5)),
        ("INDEX([nums],2)", json!(2)),
        ("MATCH(3,[nums],0)", json!(3)),
        ("SUMIFS([nums],[words2],\"a\",[nums],\">1\")", json!(6)),
        ("COUNTIFS([words2],\"a\")", json!(3)),
        ("AVERAGEIFS([nums],[words2],\"a\")", json!(7.0 / 3.0)),
        ("MINIFS([nums],[words2],\"a\")", json!(1)),
        ("MAXIFS([nums],[words2],\"b\")", json!(3)),
        ("XLOOKUP(\"b\",[words2],[nums])", json!(3)),
    ];
    assert_eq!(cases.len(), 56);
    assert_eq!(FUNCTIONS.len(), 56);
    for (name, _) in FUNCTIONS {
        assert!(
            cases
                .iter()
                .any(|(f, _)| f.starts_with(&format!("{name}("))),
            "Missing {name}"
        );
    }
    for (formula, expected) in cases {
        let actual = run(formula).unwrap_or_else(|e| panic!("{formula}: {e}"));
        if expected.is_number() {
            assert_eq!(actual.as_f64(), expected.as_f64(), "{formula}");
        } else {
            assert_eq!(actual, expected, "{formula}");
        }
    }
}
#[test]
fn rejects_invalid_syntax_arity_and_limits() {
    for s in [
        "SUM()",
        "IF(1,2)",
        "UNKNOWN(1)",
        "1 +",
        "[broken",
        "\"broken",
        "1;evil",
        "1e9999",
        "SUMIFS([nums],[words])",
        "COUNTIFS([words])",
    ] {
        assert!(parse(s).is_err(), "{s}");
    }
    assert!(parse(&"(".repeat(65)).is_err());
    assert!(parse(&"1".repeat(4097)).is_err());
    assert!(run("1/0").is_err());
    assert!(run("SQRT(-1)").is_err());
    assert!(run("DATE(2024,1,-1e300)").is_err());
    assert!(
        function(
            "SUBSTITUTE",
            &[
                &json!("a".repeat(2000)),
                &json!("a"),
                &json!("b".repeat(2000))
            ]
        )
        .is_err()
    );
    assert!(
        function(
            "TEXTJOIN",
            &[
                &json!("x".repeat(600000)),
                &json!(true),
                &json!(["a", "b", "c"])
            ]
        )
        .is_err()
    );
    assert!(run("LEFT(\"a\",-1)").is_err());
    assert!(run("INDEX([nums],0)").is_err());
    assert!(run("IFERROR(SQRT(-1),3)").is_ok());
    assert_eq!(run("2+3*4=14").unwrap(), json!(true));
    assert_eq!(run("COUNTIF([words], \"?\")").unwrap(), json!(3));
    assert_eq!(run("SEARCH(\"h*o\", \"hello\")").unwrap(), json!(1));
    assert_eq!(run("MATCH(\"b*\", [words], 0)").unwrap(), json!(2));
    assert_eq!(run("\"a\"\"b\" & UPPER(\"c\")").unwrap(), json!("a\"bC"));
}
fn kindergartens() -> (Blueprint, Lookups) {
    let bp: Blueprint = serde_json::from_value(json!({"fields":[
        {"name":"org_nr","type":"string"},
        {"name":"ar","type":"number"},
        {"name":"weighted","type":"calculated","formula":
            "=IFERROR(SUMIFS([basil-arsmelding.arsverk];[basil-arsmelding.org_nr];[org_nr];[basil-arsmelding.ar];[ar]-2);\"\")*7/12+IFERROR(SUMIFS([basil-arsmelding.arsverk];[basil-arsmelding.org_nr];[org_nr];[basil-arsmelding.ar];[ar]-1);\"\")*5/12"},
        {"name":"kind","type":"calculated","formula":"=XLOOKUP([org_nr],[basil-arsmelding.org_nr],[basil-arsmelding.type],\"unknown\")"},
        {"name":"reports","type":"calculated","formula":"=COUNTIFS([basil-arsmelding.org_nr],[org_nr])"}
    ]}))
    .unwrap();
    let rows = [
        (2022, "920143067", 3.0, "Ordinær"),
        (2023, "920143067", 3.55, "Ordinær"),
        (2023, "920062407", 27.55, "Familie"),
        // Numbers stored as text still match numeric criteria.
        (2023, "828227572", 9.33, "Ordinær"),
    ];
    let mut lookups = Lookups::default();
    let column = |values: Vec<Value>| Arc::new(LookupColumn::new("basil-arsmelding", values));
    lookups.insert(
        "basil-arsmelding",
        "ar",
        column(rows.iter().map(|r| json!(r.0)).collect()),
    );
    lookups.insert(
        "basil-arsmelding",
        "org_nr",
        column(rows.iter().map(|r| json!(r.1)).collect()),
    );
    lookups.insert(
        "basil-arsmelding",
        "arsverk",
        column(rows.iter().map(|r| json!(r.2)).collect()),
    );
    lookups.insert(
        "basil-arsmelding",
        "type",
        column(rows.iter().map(|r| json!(r.3)).collect()),
    );
    (bp, lookups)
}
#[test]
fn formulas_aggregate_columns_of_other_collections() {
    let (bp, lookups) = kindergartens();
    crate::validate_blueprint(&bp).unwrap();
    assert_eq!(
        lookup_references(&bp),
        ["arsverk", "org_nr", "ar", "type"]
            .iter()
            .map(|f| ("basil-arsmelding".to_string(), f.to_string()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    );
    let (body, errors) =
        calculate_document_with(&bp, &json!({"org_nr":"920143067","ar":2024}), &lookups);
    assert!(errors.is_empty(), "{errors:?}");
    let expected = 3.0 * 7.0 / 12.0 + 3.55 * 5.0 / 12.0;
    assert!((body["weighted"].as_f64().unwrap() - expected).abs() < 1e-9);
    assert_eq!(body["kind"], json!("Ordinær"));
    assert_eq!(body["reports"], json!(2));
    // An org number given as a number matches text values.
    let (body, _) = calculate_document_with(&bp, &json!({"org_nr":828227572,"ar":2025}), &lookups);
    assert!((body["weighted"].as_f64().unwrap() - 9.33 * 7.0 / 12.0).abs() < 1e-9);
    let (body, _) = calculate_document_with(&bp, &json!({"org_nr":"1","ar":2025}), &lookups);
    assert_eq!(body["weighted"].as_f64(), Some(0.0));
    assert_eq!(body["kind"], json!("unknown"));
    // Without the other collection's data the fields report why.
    let (body, errors) = calculate_document(&bp, &json!({"org_nr":"1","ar":2025}));
    assert_eq!(body["reports"], Value::Null);
    assert!(errors["reports"].contains("#REF!"), "{errors:?}");
}
#[test]
fn indexed_and_scanned_criteria_agree() {
    let (_, lookups) = kindergartens();
    let all = HashMap::new();
    let ctx = |body| Ctx {
        all: &all,
        body,
        lookups: &lookups,
    };
    let body = json!({"sums":[3.0,3.55,27.55,9.33],"list":[2022,2023,2023,2023],"orgs":["920143067","920143067","920062407","828227572"]});
    for (indexed, scanned) in [
        (
            "SUMIFS([basil-arsmelding.arsverk],[basil-arsmelding.ar],2023)",
            "SUMIFS([sums],[list],2023)",
        ),
        (
            "COUNTIFS([basil-arsmelding.org_nr],\"=920143067\")",
            "COUNTIF([orgs],\"=920143067\")",
        ),
        (
            "COUNTIFS([basil-arsmelding.ar],\">2022\")",
            "COUNTIF([list],\">2022\")",
        ),
        ("COUNTIFS([basil-arsmelding.type],\"ORDINÆR\")", "3"),
    ] {
        let a = eval(&parse(indexed).unwrap(), &ctx(&body), &mut HashMap::new()).unwrap();
        let b = eval(&parse(scanned).unwrap(), &ctx(&body), &mut HashMap::new()).unwrap();
        assert_eq!(a.as_f64(), b.as_f64(), "{indexed}");
    }
    let mismatched = parse("SUMIFS([basil-arsmelding.arsverk],[orgs],1,[list],2)").unwrap();
    assert!(eval(&mismatched, &ctx(&body), &mut HashMap::new()).is_err());
}
#[test]
fn cross_references_rename_and_validate() {
    let renamed = rename_formula_references("=[a.b] & \"[a.b]\" & [c]", |n| {
        n.strip_prefix("a.").map(|f| format!("z.{f}"))
    });
    assert_eq!(renamed, "=[z.b] & \"[a.b]\" & [c]");
    let bp: Blueprint = serde_json::from_value(json!({"fields":[
        {"name":"x","type":"calculated","formula":"=SUM([Bad Name.field])"}
    ]}))
    .unwrap();
    assert!(crate::validate_blueprint(&bp).is_err());
}
#[test]
fn dependencies_recalculate_and_bad_schemas_are_rejected() {
    let bp: Blueprint = serde_json::from_value(json!({"fields":[
        {"name":"net","type":"calculated","formula":"=[total] * 0.8"},
        {"name":"price","type":"number"},
        {"name":"quantity","type":"number"},
        {"name":"total","type":"calculated","formula":"=[price] * [quantity]"}
    ]}))
    .unwrap();
    crate::validate_blueprint(&bp).unwrap();
    let (body, errors) = calculate_document(&bp, &json!({"price":10,"quantity":2,"total":999}));
    assert!(errors.is_empty());
    assert_eq!(body["net"].as_f64(), Some(16.0));
    let (body, _) = calculate_document(&bp, &json!({"price":20,"quantity":2}));
    assert_eq!(body["net"].as_f64(), Some(32.0));
    let mut bad = bp.clone();
    bad.fields[3].formula = Some("=[net]".into());
    assert!(
        crate::validate_blueprint(&bad)
            .unwrap_err()
            .contains("circular")
    );
    bad.fields[3].formula = Some("=[missing]".into());
    assert!(
        crate::validate_blueprint(&bad)
            .unwrap_err()
            .contains("unknown")
    );
    let source = source_document(&bp, &body);
    assert!(source.get("net").is_none());
    assert!(source.get("total").is_none());
}
#[test]
fn calculated_numbers_read_and_round_like_spreadsheet_cells() {
    for (formula, expected) in [
        ("ROUND(1.5*2,0) & \" kr\"", json!("3 kr")),
        ("(0.1+0.2) & \"\"", json!("0.3")),
        ("CONCAT(2.5*2, \"/\", -0.0)", json!("5/0")),
        ("LEN(1.5*2)", json!(1)),
        ("ROUND(1.005,2)", json!(1.01)),
        ("ROUNDUP(0.1*3,1)", json!(0.3)),
        ("ROUNDDOWN(0.7+0.1,1)", json!(0.8)),
        ("FLOOR(0.3,0.1)", json!(0.3)),
        ("CEILING(0.3,0.1)", json!(0.3)),
        ("ROUND(1.5,308)", json!(1.5)),
    ] {
        assert_eq!(run(formula), Ok(expected), "{formula}");
    }
}
#[test]
fn comparisons_read_numeric_text_and_empty_values_as_numbers() {
    let ctx = |body| {
        let expr = parse("IF([year] < 2025, \"old\", \"new\")").unwrap();
        eval(
            &expr,
            &Ctx {
                all: &HashMap::new(),
                body: &body,
                lookups: &Lookups::default(),
            },
            &mut HashMap::new(),
        )
    };
    assert_eq!(ctx(json!({"year": "999"})), Ok(json!("old")));
    assert_eq!(ctx(json!({"year": "2026"})), Ok(json!("new")));
    for (formula, expected) in [
        ("\"9\" < 10", true),
        ("\"10\" > \"9\"", true),
        ("\" 7 \" = 7", true),
        ("[missing] = 0", true),
        ("[missing] > -5", true),
        ("[missing] = \"\"", true),
        ("\"apple\" < \"Banana\"", true),
        ("\"Example\" < 10", false),
        ("TRUE = TRUE", true),
    ] {
        assert_eq!(run(formula), Ok(json!(expected)), "{formula}");
    }
}
#[test]
fn functions_read_large_columns_of_other_collections() {
    // Serialized, this column is well over the 1 MB limit on built values.
    let mut lookups = Lookups::default();
    let values: Vec<Value> = (0..200_000).map(|n| json!(n * 1001)).collect();
    lookups.insert(
        "orders",
        "amount",
        Arc::new(LookupColumn::new("orders", values)),
    );
    let ctx = Ctx {
        all: &HashMap::new(),
        body: &json!({}),
        lookups: &lookups,
    };
    for (formula, expected) in [
        ("SUM([orders.amount])", json!(20_019_899_900_000.0)),
        ("COUNT([orders.amount])", json!(200_000)),
        ("MAX([orders.amount])", json!(200_198_999.0)),
        ("COUNTIF([orders.amount], \">100000000\")", json!(100_099)),
        ("INDEX([orders.amount], 3)", json!(2002)),
    ] {
        let expr = parse(formula).unwrap();
        assert_eq!(
            eval(&expr, &ctx, &mut HashMap::new()),
            Ok(expected),
            "{formula}"
        );
    }
}
