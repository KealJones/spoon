//! Behavioral tests for the collection, set, and map extras of the intrinsic
//! vocabulary.
//!
//! Learned procedures call these operations directly, so each case pins down
//! something a procedure can depend on: the exact value produced, the exact
//! error a bad argument produces, and whether the output is reproducible
//! across runs. Nondeterministic ordering or an unbounded allocation here
//! would surface as an irreproducible or unbounded procedure, which is far
//! harder to diagnose from the outside.
//!
//! Where the implementation contradicts the published description of an
//! operation (`packages/inspector/src/server.ts` is what a teacher and a
//! reader are told these ops do), the test asserts the description and fails
//! on purpose rather than freezing the defect.

use spoon_core::{Expr, IntrinsicOp, Value};
use spoon_exec::{Env, Evaluator, SpoonError};

/// `MAX_INTRINSIC_ITEMS` in `spoon-exec/src/eval.rs`, duplicated because it is
/// private. The tests that depend on it assert the limit value too, so a
/// change to the constant surfaces here instead of silently passing.
const MAX_ITEMS: usize = 100_000;

fn intrinsic(op: IntrinsicOp, args: Vec<Expr>) -> Expr {
    Expr::Intrinsic {
        version: 1,
        op,
        args,
    }
}

fn lit(value: Value) -> Expr {
    Expr::Literal(value)
}

fn lit_int(n: i64) -> Expr {
    Expr::Literal(Value::Int(n))
}

fn lit_text(text: &str) -> Expr {
    Expr::Literal(Value::Text(text.to_string()))
}

fn lit_list(items: Vec<Value>) -> Expr {
    Expr::Literal(Value::List(items))
}

fn text(value: &str) -> Value {
    Value::Text(value.to_string())
}

fn list(items: Vec<Value>) -> Value {
    Value::List(items)
}

fn map(entries: Vec<(&str, Value)>) -> Value {
    Value::Map(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
    )
}

fn eval(expr: &Expr) -> Result<Value, SpoonError> {
    Evaluator::new().eval(expr, &mut Env::new())
}

fn eval_ok(expr: &Expr) -> Value {
    eval(expr).expect("expression evaluates")
}

fn eval_err(expr: &Expr) -> SpoonError {
    eval(expr).expect_err("expression fails")
}

fn assert_type_error(error: SpoonError, expected_type: &str) {
    match error {
        SpoonError::TypeError { expected, .. } => assert_eq!(expected, expected_type),
        other => panic!("expected a {expected_type} type error, got {other:?}"),
    }
}

fn assert_arity_error(error: SpoonError, op_name: &str, expected_arity: usize, got_arity: usize) {
    match error {
        SpoonError::ArityMismatch {
            name,
            expected,
            got,
        } => {
            assert_eq!(name, op_name);
            assert_eq!(expected, expected_arity);
            assert_eq!(got, got_arity);
        }
        other => panic!("expected an arity mismatch for {op_name}, got {other:?}"),
    }
}

fn assert_message_contains(error: SpoonError, needle: &str) {
    match error {
        SpoonError::Other(ref message) => assert!(
            message.contains(needle),
            "expected a message containing {needle:?}, got {message:?}"
        ),
        other => panic!("expected an Other error containing {needle:?}, got {other:?}"),
    }
}

fn assert_limit_exceeded(error: SpoonError, expected_operation: &str, expected_limit: usize) {
    match error {
        SpoonError::IntrinsicLimitExceeded { operation, limit } => {
            assert_eq!(operation, expected_operation);
            assert_eq!(limit, expected_limit);
        }
        other => panic!("expected a limit error for {expected_operation}, got {other:?}"),
    }
}

/// The keys of a map result in iteration order, so a test can assert the
/// order itself rather than only set equality.
fn map_keys(value: &Value) -> Vec<String> {
    match value {
        Value::Map(entries) => entries.keys().cloned().collect(),
        other => panic!("expected a map, got {other:?}"),
    }
}

// -- collection_first / collection_last --

#[test]
fn collection_first_and_last_return_the_boundary_elements() {
    let items = vec![Value::Int(10), Value::Int(20), Value::Int(30)];

    let first = eval_ok(&intrinsic(
        IntrinsicOp::CollectionFirst,
        vec![lit_list(items.clone())],
    ));
    let last = eval_ok(&intrinsic(
        IntrinsicOp::CollectionLast,
        vec![lit_list(items)],
    ));

    assert_eq!(first, Value::Int(10));
    assert_eq!(last, Value::Int(30));
}

#[test]
fn collection_first_and_last_return_a_single_element_list_unchanged() {
    let single = vec![text("only")];

    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::CollectionFirst,
            vec![lit_list(single.clone())]
        )),
        text("only")
    );
    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::CollectionLast,
            vec![lit_list(single)]
        )),
        text("only")
    );
}

#[test]
fn collection_first_and_last_error_on_an_empty_list_rather_than_returning_null() {
    // A null return would be indistinguishable from a stored null element, so
    // a procedure could not tell "no elements" from "first element is null".
    assert_message_contains(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionFirst,
            vec![lit_list(vec![])],
        )),
        "collection_first: list must not be empty",
    );
    assert_message_contains(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionLast,
            vec![lit_list(vec![])],
        )),
        "collection_last: list must not be empty",
    );
}

#[test]
fn collection_first_preserves_a_stored_null_as_a_successful_result() {
    let result = eval_ok(&intrinsic(
        IntrinsicOp::CollectionFirst,
        vec![lit_list(vec![Value::Null, Value::Int(1)])],
    ));

    assert_eq!(result, Value::Null);
}

#[test]
fn collection_first_rejects_a_non_list_argument_and_a_wrong_argument_count() {
    assert_type_error(
        eval_err(&intrinsic(IntrinsicOp::CollectionFirst, vec![lit_int(1)])),
        "list",
    );
    assert_arity_error(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionLast,
            vec![lit_list(vec![]), lit_int(1)],
        )),
        "collection_last",
        1,
        2,
    );
}

// -- collection_take / collection_drop --

#[test]
fn collection_take_and_drop_split_a_list_at_the_requested_count() {
    let items = vec![Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)];

    let taken = eval_ok(&intrinsic(
        IntrinsicOp::CollectionTake,
        vec![lit_list(items.clone()), lit_int(2)],
    ));
    let dropped = eval_ok(&intrinsic(
        IntrinsicOp::CollectionDrop,
        vec![lit_list(items), lit_int(2)],
    ));

    assert_eq!(taken, list(vec![Value::Int(1), Value::Int(2)]));
    assert_eq!(dropped, list(vec![Value::Int(3), Value::Int(4)]));
}

#[test]
fn collection_take_and_drop_saturate_instead_of_erroring_on_out_of_range_counts() {
    let items = vec![Value::Int(1), Value::Int(2)];
    let take = |n: i64| {
        eval_ok(&intrinsic(
            IntrinsicOp::CollectionTake,
            vec![lit_list(items.clone()), lit_int(n)],
        ))
    };
    let drop = |n: i64| {
        eval_ok(&intrinsic(
            IntrinsicOp::CollectionDrop,
            vec![lit_list(items.clone()), lit_int(n)],
        ))
    };

    assert_eq!(take(0), list(vec![]));
    assert_eq!(take(2), list(items.clone()));
    assert_eq!(take(99), list(items.clone()));
    assert_eq!(drop(0), list(items.clone()));
    assert_eq!(drop(2), list(vec![]));
    assert_eq!(drop(99), list(vec![]));
}

#[test]
fn collection_take_and_drop_of_an_empty_list_are_empty_for_any_valid_count() {
    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::CollectionTake,
            vec![lit_list(vec![]), lit_int(3)]
        )),
        list(vec![])
    );
    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::CollectionDrop,
            vec![lit_list(vec![]), lit_int(3)]
        )),
        list(vec![])
    );
}

#[test]
fn collection_take_and_drop_reject_a_negative_count() {
    // A negative count must not wrap into a huge usize, which is what makes
    // this worth asserting rather than assuming.
    assert_message_contains(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionTake,
            vec![lit_list(vec![Value::Int(1)]), lit_int(-1)],
        )),
        "collection_take n must be non-negative",
    );
    assert_message_contains(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionDrop,
            vec![lit_list(vec![Value::Int(1)]), lit_int(-3)],
        )),
        "collection_drop n must be non-negative",
    );
}

#[test]
fn collection_take_rejects_a_non_integer_count_and_a_wrong_argument_count() {
    assert_type_error(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionTake,
            vec![lit_list(vec![]), lit_text("2")],
        )),
        "int",
    );
    assert_arity_error(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionTake,
            vec![lit_list(vec![])],
        )),
        "collection_take",
        2,
        1,
    );
}

// -- collection_chunk --

#[test]
fn collection_chunk_groups_elements_and_keeps_a_short_final_chunk() {
    let items = vec![
        Value::Int(1),
        Value::Int(2),
        Value::Int(3),
        Value::Int(4),
        Value::Int(5),
    ];

    let result = eval_ok(&intrinsic(
        IntrinsicOp::CollectionChunk,
        vec![lit_list(items), lit_int(2)],
    ));

    assert_eq!(
        result,
        list(vec![
            list(vec![Value::Int(1), Value::Int(2)]),
            list(vec![Value::Int(3), Value::Int(4)]),
            list(vec![Value::Int(5)]),
        ])
    );
}

#[test]
fn collection_chunk_of_size_one_wraps_each_element_and_an_oversized_chunk_keeps_the_list_whole() {
    let items = vec![Value::Int(1), Value::Int(2)];

    let singles = eval_ok(&intrinsic(
        IntrinsicOp::CollectionChunk,
        vec![lit_list(items.clone()), lit_int(1)],
    ));
    let oversized = eval_ok(&intrinsic(
        IntrinsicOp::CollectionChunk,
        vec![lit_list(items.clone()), lit_int(5)],
    ));

    assert_eq!(
        singles,
        list(vec![list(vec![Value::Int(1)]), list(vec![Value::Int(2)])])
    );
    assert_eq!(oversized, list(vec![list(items)]));
}

#[test]
fn collection_chunk_of_an_empty_list_produces_no_chunks() {
    let result = eval_ok(&intrinsic(
        IntrinsicOp::CollectionChunk,
        vec![lit_list(vec![]), lit_int(2)],
    ));

    assert_eq!(result, list(vec![]));
}

#[test]
fn collection_chunk_rejects_a_zero_size_before_it_can_loop_forever() {
    // `slice::chunks(0)` panics, so the guard has to run before the split.
    // The zero case is the classic way this operation hangs or aborts.
    assert_message_contains(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionChunk,
            vec![lit_list(vec![Value::Int(1)]), lit_int(0)],
        )),
        "collection_chunk: chunk size must be > 0",
    );
}

#[test]
fn collection_chunk_rejects_a_negative_size() {
    assert_message_contains(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionChunk,
            vec![lit_list(vec![Value::Int(1)]), lit_int(-2)],
        )),
        "collection_chunk n must be non-negative",
    );
}

// -- collection_window --

#[test]
fn collection_window_slides_over_every_consecutive_run() {
    let items = vec![Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)];

    let result = eval_ok(&intrinsic(
        IntrinsicOp::CollectionWindow,
        vec![lit_list(items), lit_int(2)],
    ));

    assert_eq!(
        result,
        list(vec![
            list(vec![Value::Int(1), Value::Int(2)]),
            list(vec![Value::Int(2), Value::Int(3)]),
            list(vec![Value::Int(3), Value::Int(4)]),
        ])
    );
}

#[test]
fn collection_window_of_size_one_yields_each_element_and_of_the_full_length_yields_one_window() {
    let items = vec![Value::Int(7), Value::Int(8)];

    let singles = eval_ok(&intrinsic(
        IntrinsicOp::CollectionWindow,
        vec![lit_list(items.clone()), lit_int(1)],
    ));
    let whole = eval_ok(&intrinsic(
        IntrinsicOp::CollectionWindow,
        vec![lit_list(items.clone()), lit_int(2)],
    ));

    assert_eq!(
        singles,
        list(vec![list(vec![Value::Int(7)]), list(vec![Value::Int(8)])])
    );
    assert_eq!(whole, list(vec![list(items)]));
}

#[test]
fn collection_window_yields_nothing_rather_than_erroring_when_the_size_is_zero_or_too_large() {
    // Documented behavior for a procedure author: an over-long window is not
    // an error, it is an empty result, so a caller must handle the empty list
    // rather than expecting a failure it can catch. `slice::windows(0)`
    // panics, so the zero guard is load bearing.
    let items = vec![Value::Int(1), Value::Int(2)];

    let zero = eval_ok(&intrinsic(
        IntrinsicOp::CollectionWindow,
        vec![lit_list(items.clone()), lit_int(0)],
    ));
    let too_large = eval_ok(&intrinsic(
        IntrinsicOp::CollectionWindow,
        vec![lit_list(items), lit_int(3)],
    ));
    let empty_input = eval_ok(&intrinsic(
        IntrinsicOp::CollectionWindow,
        vec![lit_list(vec![]), lit_int(1)],
    ));

    assert_eq!(zero, list(vec![]));
    assert_eq!(too_large, list(vec![]));
    assert_eq!(empty_input, list(vec![]));
}

#[test]
fn collection_window_rejects_a_negative_size_and_a_non_list_input() {
    assert_message_contains(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionWindow,
            vec![lit_list(vec![Value::Int(1)]), lit_int(-1)],
        )),
        "collection_window n must be non-negative",
    );
    assert_type_error(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionWindow,
            vec![lit_text("abc"), lit_int(1)],
        )),
        "list",
    );
}

// -- collection_enumerate --

#[test]
fn collection_enumerate_pairs_each_element_with_its_zero_based_index() {
    let result = eval_ok(&intrinsic(
        IntrinsicOp::CollectionEnumerate,
        vec![lit_list(vec![text("a"), text("b"), text("c")])],
    ));

    assert_eq!(
        result,
        list(vec![
            list(vec![Value::Int(0), text("a")]),
            list(vec![Value::Int(1), text("b")]),
            list(vec![Value::Int(2), text("c")]),
        ])
    );
}

#[test]
fn collection_enumerate_of_an_empty_list_is_empty() {
    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::CollectionEnumerate,
            vec![lit_list(vec![])]
        )),
        list(vec![])
    );
}

#[test]
fn collection_enumerate_rejects_a_non_list_argument_and_a_wrong_argument_count() {
    assert_type_error(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionEnumerate,
            vec![lit(map(vec![("a", Value::Int(1))]))],
        )),
        "list",
    );
    assert_arity_error(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionEnumerate,
            vec![lit_list(vec![]), lit_int(1)],
        )),
        "collection_enumerate",
        1,
        2,
    );
}

// -- collection_repeat_value --

#[test]
fn collection_repeat_value_copies_a_value_the_requested_number_of_times() {
    let result = eval_ok(&intrinsic(
        IntrinsicOp::CollectionRepeatValue,
        vec![lit_text("x"), lit_int(3)],
    ));

    assert_eq!(result, list(vec![text("x"), text("x"), text("x")]));
}

#[test]
fn collection_repeat_value_of_zero_copies_is_an_empty_list() {
    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::CollectionRepeatValue,
            vec![lit_text("x"), lit_int(0)]
        )),
        list(vec![])
    );
}

#[test]
fn collection_repeat_value_repeats_structured_values_without_flattening_them() {
    let result = eval_ok(&intrinsic(
        IntrinsicOp::CollectionRepeatValue,
        vec![lit(map(vec![("k", Value::Int(1))])), lit_int(2)],
    ));

    assert_eq!(
        result,
        list(vec![
            map(vec![("k", Value::Int(1))]),
            map(vec![("k", Value::Int(1))]),
        ])
    );
}

#[test]
fn collection_repeat_value_is_bounded_by_the_item_limit_so_it_cannot_be_an_allocation_bomb() {
    // Without this bound a single learned expression could ask for billions of
    // copies and exhaust host memory. The check must reject before allocating.
    let error = eval_err(&intrinsic(
        IntrinsicOp::CollectionRepeatValue,
        vec![lit_int(0), lit_int(MAX_ITEMS as i64 + 1)],
    ));

    assert_limit_exceeded(error, "collection_repeat_value output items", MAX_ITEMS);
}

#[test]
fn collection_repeat_value_is_also_bounded_by_output_size_when_the_repeated_value_is_large() {
    // The item count alone is not enough: 2_000 copies of a 1_000 byte value
    // is under the item limit but still two megabytes of output.
    let error = eval_err(&intrinsic(
        IntrinsicOp::CollectionRepeatValue,
        vec![lit_text(&"a".repeat(1_000)), lit_int(2_000)],
    ));

    assert_limit_exceeded(error, "collection_repeat_value output bytes", 1_048_576);
}

#[test]
fn collection_repeat_value_rejects_a_negative_count() {
    assert_message_contains(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionRepeatValue,
            vec![lit_int(1), lit_int(-1)],
        )),
        "collection_repeat_value n must be non-negative",
    );
}

// -- collection_all / collection_any --

#[test]
fn collection_all_and_any_fold_element_truthiness_rather_than_applying_a_predicate() {
    // These take one argument: there is no predicate to pass, so a procedure
    // has to map first and reduce second.
    let all_truthy = vec![Value::Bool(true), Value::Int(1), text("x")];
    // An empty list is a falsy element, which is easy to trip over when the
    // input is itself a list of lists.
    let mixed = vec![Value::Bool(true), list(vec![])];

    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::CollectionAll,
            vec![lit_list(all_truthy.clone())]
        )),
        Value::Bool(true)
    );
    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::CollectionAny,
            vec![lit_list(all_truthy)]
        )),
        Value::Bool(true)
    );
    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::CollectionAll,
            vec![lit_list(mixed.clone())]
        )),
        Value::Bool(false)
    );
    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::CollectionAny,
            vec![lit_list(mixed)]
        )),
        Value::Bool(true)
    );
}

#[test]
fn collection_all_and_any_treat_every_empty_or_zero_value_as_falsy() {
    let falsy = vec![
        Value::Null,
        Value::Bool(false),
        Value::Int(0),
        Value::Float(0.0),
        text(""),
        list(vec![]),
        map(vec![]),
    ];

    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::CollectionAny,
            vec![lit_list(falsy.clone())]
        )),
        Value::Bool(false)
    );
    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::CollectionAll,
            vec![lit_list(falsy)]
        )),
        Value::Bool(false)
    );
}

#[test]
fn collection_all_is_vacuously_true_and_collection_any_is_false_on_an_empty_list() {
    // The standard vacuous truth convention. Getting this backwards would
    // silently invert every guard a procedure builds on top of it.
    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::CollectionAll,
            vec![lit_list(vec![])]
        )),
        Value::Bool(true)
    );
    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::CollectionAny,
            vec![lit_list(vec![])]
        )),
        Value::Bool(false)
    );
}

#[test]
fn collection_all_rejects_a_non_list_argument_and_collection_any_rejects_a_wrong_argument_count() {
    assert_type_error(
        eval_err(&intrinsic(IntrinsicOp::CollectionAll, vec![lit_text("x")])),
        "list",
    );
    assert_arity_error(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionAny,
            vec![lit_list(vec![]), lit_list(vec![])],
        )),
        "collection_any",
        1,
        2,
    );
}

// -- collection_sort_by --

/// The key argument of the `*_by` operations is a field name, not a callable,
/// so every fixture here is a list of maps.
fn scored(id: &str, score: Value) -> Value {
    map(vec![("id", text(id)), ("score", score)])
}

#[test]
fn collection_sort_by_orders_maps_by_the_named_field() {
    let items = vec![
        scored("a", Value::Int(30)),
        scored("b", Value::Int(10)),
        scored("c", Value::Int(20)),
    ];

    let result = eval_ok(&intrinsic(
        IntrinsicOp::CollectionSortBy,
        vec![lit_list(items), lit_text("score")],
    ));

    assert_eq!(
        result,
        list(vec![
            scored("b", Value::Int(10)),
            scored("c", Value::Int(20)),
            scored("a", Value::Int(30)),
        ])
    );
}

#[test]
fn collection_sort_by_is_stable_so_equal_keys_keep_their_input_order() {
    // Stability is what makes a multi pass sort usable, and it is also what
    // makes the result reproducible for a replayed procedure.
    let items = vec![
        scored("first", Value::Int(1)),
        scored("second", Value::Int(1)),
        scored("third", Value::Int(0)),
    ];

    let result = eval_ok(&intrinsic(
        IntrinsicOp::CollectionSortBy,
        vec![lit_list(items), lit_text("score")],
    ));

    assert_eq!(
        result,
        list(vec![
            scored("third", Value::Int(0)),
            scored("first", Value::Int(1)),
            scored("second", Value::Int(1)),
        ])
    );
}

#[test]
fn collection_sort_by_orders_mixed_key_types_by_type_rank_instead_of_panicking() {
    // The comparator ranks types (null, bool, int, float, text, list, map)
    // before comparing within a type, so a heterogeneous column still has a
    // total order. Note that int outranks float: a column mixing 5 and 1.0
    // does NOT come out in numeric order.
    let items = vec![
        scored("text", text("b")),
        scored("int", Value::Int(5)),
        scored("null", Value::Null),
        scored("bool", Value::Bool(true)),
        scored("float", Value::Float(1.0)),
    ];

    let result = eval_ok(&intrinsic(
        IntrinsicOp::CollectionSortBy,
        vec![lit_list(items), lit_text("score")],
    ));

    assert_eq!(
        result,
        list(vec![
            scored("null", Value::Null),
            scored("bool", Value::Bool(true)),
            scored("int", Value::Int(5)),
            scored("float", Value::Float(1.0)),
            scored("text", text("b")),
        ])
    );
}

#[test]
fn collection_sort_by_treats_a_missing_key_and_a_non_map_element_as_null() {
    // Silent coercion rather than an error, so a malformed row sorts to the
    // front instead of failing the procedure. Worth pinning down because the
    // quiet path is the surprising one.
    let items = vec![
        scored("has", Value::Int(1)),
        map(vec![("id", text("missing"))]),
        Value::Int(99),
    ];

    let result = eval_ok(&intrinsic(
        IntrinsicOp::CollectionSortBy,
        vec![lit_list(items), lit_text("score")],
    ));

    assert_eq!(
        result,
        list(vec![
            map(vec![("id", text("missing"))]),
            Value::Int(99),
            scored("has", Value::Int(1)),
        ])
    );
}

#[test]
fn collection_sort_by_of_an_empty_list_is_empty() {
    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::CollectionSortBy,
            vec![lit_list(vec![]), lit_text("score")]
        )),
        list(vec![])
    );
}

#[test]
fn collection_sort_by_rejects_a_non_text_key_and_a_wrong_argument_count() {
    assert_type_error(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionSortBy,
            vec![lit_list(vec![]), lit_int(0)],
        )),
        "text",
    );
    assert_arity_error(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionSortBy,
            vec![lit_list(vec![])],
        )),
        "collection_sort_by",
        2,
        1,
    );
}

// -- collection_group_by --

#[test]
fn collection_group_by_collects_colliding_keys_in_input_order() {
    let items = vec![
        map(vec![("team", text("a")), ("n", Value::Int(1))]),
        map(vec![("team", text("b")), ("n", Value::Int(2))]),
        map(vec![("team", text("a")), ("n", Value::Int(3))]),
    ];

    let result = eval_ok(&intrinsic(
        IntrinsicOp::CollectionGroupBy,
        vec![lit_list(items), lit_text("team")],
    ));

    assert_eq!(
        result,
        map(vec![
            (
                "a",
                list(vec![
                    map(vec![("team", text("a")), ("n", Value::Int(1))]),
                    map(vec![("team", text("a")), ("n", Value::Int(3))]),
                ])
            ),
            (
                "b",
                list(vec![map(vec![("team", text("b")), ("n", Value::Int(2))])])
            ),
        ])
    );
}

#[test]
fn collection_group_by_produces_the_same_ordering_on_every_run() {
    // Nondeterministic grouping order would make any procedure built on this
    // irreproducible, which defeats replay based credit assignment.
    let items = vec![
        map(vec![("k", text("z"))]),
        map(vec![("k", text("a"))]),
        map(vec![("k", text("m"))]),
    ];
    let expression = intrinsic(
        IntrinsicOp::CollectionGroupBy,
        vec![lit_list(items), lit_text("k")],
    );

    let first = eval_ok(&expression);
    let second = eval_ok(&expression);

    assert_eq!(first, second);
    assert_eq!(map_keys(&first), vec!["a", "m", "z"]);
}

#[test]
fn collection_group_by_coerces_int_and_bool_keys_to_their_text_form() {
    let items = vec![
        map(vec![("k", Value::Int(2))]),
        map(vec![("k", Value::Bool(true))]),
    ];

    let result = eval_ok(&intrinsic(
        IntrinsicOp::CollectionGroupBy,
        vec![lit_list(items), lit_text("k")],
    ));

    assert_eq!(map_keys(&result), vec!["2", "true"]);
}

#[test]
fn collection_group_by_merges_an_absent_key_and_an_explicit_null_into_one_group() {
    // Both land in the literal group named "null", so a procedure cannot tell
    // "field missing" from "field is null" after grouping.
    let items = vec![
        map(vec![("k", Value::Null)]),
        map(vec![("other", Value::Int(1))]),
    ];

    let result = eval_ok(&intrinsic(
        IntrinsicOp::CollectionGroupBy,
        vec![lit_list(items), lit_text("k")],
    ));

    assert_eq!(map_keys(&result), vec!["null"]);
    assert_eq!(
        result,
        map(vec![(
            "null",
            list(vec![
                map(vec![("k", Value::Null)]),
                map(vec![("other", Value::Int(1))]),
            ])
        )])
    );
}

#[test]
fn collection_group_by_of_an_empty_list_is_an_empty_map() {
    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::CollectionGroupBy,
            vec![lit_list(vec![]), lit_text("k")]
        )),
        map(vec![])
    );
}

#[test]
fn collection_group_by_rejects_non_map_elements_and_unstringable_key_values() {
    assert_type_error(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionGroupBy,
            vec![lit_list(vec![Value::Int(1)]), lit_text("k")],
        )),
        "map",
    );
    assert_type_error(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionGroupBy,
            vec![
                lit_list(vec![map(vec![("k", Value::Float(1.5))])]),
                lit_text("k"),
            ],
        )),
        "text or int",
    );
}

// -- collection_partition --

#[test]
fn collection_partition_splits_on_equality_with_a_value_not_on_a_predicate() {
    // The second argument is a value compared with `==`, so there is no
    // callable to pass and no way to express a range test here.
    let items = vec![Value::Int(1), Value::Int(2), Value::Int(1), Value::Int(3)];

    let result = eval_ok(&intrinsic(
        IntrinsicOp::CollectionPartition,
        vec![lit_list(items), lit_int(1)],
    ));

    assert_eq!(
        result,
        list(vec![
            list(vec![Value::Int(1), Value::Int(1)]),
            list(vec![Value::Int(2), Value::Int(3)]),
        ])
    );
}

#[test]
fn collection_partition_compares_by_type_as_well_as_value() {
    let result = eval_ok(&intrinsic(
        IntrinsicOp::CollectionPartition,
        vec![
            lit_list(vec![Value::Int(1), Value::Float(1.0)]),
            lit(Value::Float(1.0)),
        ],
    ));

    assert_eq!(
        result,
        list(vec![
            list(vec![Value::Float(1.0)]),
            list(vec![Value::Int(1)]),
        ])
    );
}

#[test]
fn collection_partition_always_returns_two_lists_even_when_one_side_is_empty() {
    let empty = eval_ok(&intrinsic(
        IntrinsicOp::CollectionPartition,
        vec![lit_list(vec![]), lit_int(1)],
    ));
    let no_matches = eval_ok(&intrinsic(
        IntrinsicOp::CollectionPartition,
        vec![lit_list(vec![Value::Int(2)]), lit_int(1)],
    ));
    let all_matches = eval_ok(&intrinsic(
        IntrinsicOp::CollectionPartition,
        vec![lit_list(vec![Value::Int(1)]), lit_int(1)],
    ));

    assert_eq!(empty, list(vec![list(vec![]), list(vec![])]));
    assert_eq!(
        no_matches,
        list(vec![list(vec![]), list(vec![Value::Int(2)])])
    );
    assert_eq!(
        all_matches,
        list(vec![list(vec![Value::Int(1)]), list(vec![])])
    );
}

#[test]
fn collection_partition_produces_the_same_ordering_on_every_run() {
    let expression = intrinsic(
        IntrinsicOp::CollectionPartition,
        vec![
            lit_list(vec![text("b"), text("a"), text("b")]),
            lit_text("b"),
        ],
    );

    assert_eq!(eval_ok(&expression), eval_ok(&expression));
}

#[test]
fn collection_partition_rejects_a_non_list_input_and_a_wrong_argument_count() {
    assert_type_error(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionPartition,
            vec![lit_int(1), lit_int(1)],
        )),
        "list",
    );
    assert_arity_error(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionPartition,
            vec![lit_list(vec![])],
        )),
        "collection_partition",
        2,
        1,
    );
}

// -- collection_min_by / collection_max_by --

#[test]
fn collection_min_by_and_max_by_return_the_whole_element_at_the_extreme_key() {
    let items = vec![
        scored("mid", Value::Int(2)),
        scored("low", Value::Int(1)),
        scored("high", Value::Int(3)),
    ];

    let min = eval_ok(&intrinsic(
        IntrinsicOp::CollectionMinBy,
        vec![lit_list(items.clone()), lit_text("score")],
    ));
    let max = eval_ok(&intrinsic(
        IntrinsicOp::CollectionMaxBy,
        vec![lit_list(items), lit_text("score")],
    ));

    assert_eq!(min, scored("low", Value::Int(1)));
    assert_eq!(max, scored("high", Value::Int(3)));
}

#[test]
fn collection_min_by_breaks_ties_toward_the_first_element_and_max_by_toward_the_last() {
    // Deterministic but asymmetric, inherited from `Iterator::min_by` and
    // `max_by`. A procedure that expects "first wins" for both would be wrong
    // half the time, so the asymmetry is worth stating.
    let items = vec![
        scored("first", Value::Int(5)),
        scored("second", Value::Int(5)),
    ];

    let min = eval_ok(&intrinsic(
        IntrinsicOp::CollectionMinBy,
        vec![lit_list(items.clone()), lit_text("score")],
    ));
    let max = eval_ok(&intrinsic(
        IntrinsicOp::CollectionMaxBy,
        vec![lit_list(items), lit_text("score")],
    ));

    assert_eq!(min, scored("first", Value::Int(5)));
    assert_eq!(max, scored("second", Value::Int(5)));
}

#[test]
fn collection_min_by_and_max_by_error_on_an_empty_list() {
    assert_message_contains(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionMinBy,
            vec![lit_list(vec![]), lit_text("score")],
        )),
        "collection_min_by: list must not be empty",
    );
    assert_message_contains(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionMaxBy,
            vec![lit_list(vec![]), lit_text("score")],
        )),
        "collection_max_by: list must not be empty",
    );
}

#[test]
fn collection_min_by_treats_a_missing_key_as_the_smallest_value() {
    let items = vec![
        scored("has", Value::Int(-100)),
        map(vec![("id", text("missing"))]),
    ];

    let result = eval_ok(&intrinsic(
        IntrinsicOp::CollectionMinBy,
        vec![lit_list(items), lit_text("score")],
    ));

    assert_eq!(result, map(vec![("id", text("missing"))]));
}

#[test]
fn collection_max_by_rejects_a_non_text_key_and_a_wrong_argument_count() {
    assert_type_error(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionMaxBy,
            vec![lit_list(vec![]), lit_list(vec![])],
        )),
        "text",
    );
    assert_arity_error(
        eval_err(&intrinsic(
            IntrinsicOp::CollectionMinBy,
            vec![lit_list(vec![]), lit_text("k"), lit_text("k")],
        )),
        "collection_min_by",
        2,
        3,
    );
}

// -- set_union / set_intersect / set_difference / set_is_subset --

#[test]
fn set_union_keeps_the_left_order_then_appends_new_right_elements() {
    let result = eval_ok(&intrinsic(
        IntrinsicOp::SetUnion,
        vec![
            lit_list(vec![Value::Int(1), Value::Int(2)]),
            lit_list(vec![Value::Int(2), Value::Int(3)]),
        ],
    ));

    assert_eq!(
        result,
        list(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
    );
}

#[test]
fn set_union_produces_unique_elements_from_both_lists() {
    // `set_union` is published as "Unique elements from both lists"
    // (packages/inspector/src/server.ts:916). The implementation only
    // deduplicates the right operand against the accumulator, so duplicates
    // inside the left operand survive and the result depends on which side an
    // element arrives from. That breaks the commutativity a procedure author
    // will assume from the name.
    let left_duplicates = eval_ok(&intrinsic(
        IntrinsicOp::SetUnion,
        vec![
            lit_list(vec![Value::Int(1), Value::Int(1), Value::Int(2)]),
            lit_list(vec![Value::Int(2)]),
        ],
    ));
    let right_duplicates = eval_ok(&intrinsic(
        IntrinsicOp::SetUnion,
        vec![
            lit_list(vec![Value::Int(1)]),
            lit_list(vec![Value::Int(2), Value::Int(2)]),
        ],
    ));

    assert_eq!(
        left_duplicates,
        list(vec![Value::Int(1), Value::Int(2)]),
        "duplicates in the left operand must not survive a set union"
    );
    assert_eq!(right_duplicates, list(vec![Value::Int(1), Value::Int(2)]));
}

#[test]
fn set_union_of_two_empty_lists_is_empty_and_a_union_with_empty_is_the_other_side() {
    let both_empty = eval_ok(&intrinsic(
        IntrinsicOp::SetUnion,
        vec![lit_list(vec![]), lit_list(vec![])],
    ));
    let left_empty = eval_ok(&intrinsic(
        IntrinsicOp::SetUnion,
        vec![lit_list(vec![]), lit_list(vec![Value::Int(1)])],
    ));

    assert_eq!(both_empty, list(vec![]));
    assert_eq!(left_empty, list(vec![Value::Int(1)]));
}

#[test]
fn set_union_produces_the_same_ordering_on_every_run() {
    let expression = intrinsic(
        IntrinsicOp::SetUnion,
        vec![
            lit_list(vec![text("z"), text("a")]),
            lit_list(vec![text("m"), text("a")]),
        ],
    );

    let first = eval_ok(&expression);

    assert_eq!(first, eval_ok(&expression));
    // Insertion order, not sorted order: a procedure that needs sorted output
    // has to sort explicitly.
    assert_eq!(first, list(vec![text("z"), text("a"), text("m")]));
}

#[test]
fn set_intersect_and_set_difference_filter_the_left_list_and_keep_its_order() {
    let intersect = eval_ok(&intrinsic(
        IntrinsicOp::SetIntersect,
        vec![
            lit_list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
            lit_list(vec![Value::Int(3), Value::Int(1)]),
        ],
    ));
    let difference = eval_ok(&intrinsic(
        IntrinsicOp::SetDifference,
        vec![
            lit_list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
            lit_list(vec![Value::Int(2)]),
        ],
    ));

    assert_eq!(intersect, list(vec![Value::Int(1), Value::Int(3)]));
    assert_eq!(difference, list(vec![Value::Int(1), Value::Int(3)]));
}

#[test]
fn set_intersect_and_set_difference_are_filters_so_left_duplicates_are_preserved() {
    // Unlike `set_union` these are published as element filters rather than as
    // producing unique output, so multiset behavior on the left is the
    // contract. Asserting it keeps the inconsistency with `set_union` visible.
    let intersect = eval_ok(&intrinsic(
        IntrinsicOp::SetIntersect,
        vec![
            lit_list(vec![Value::Int(1), Value::Int(1), Value::Int(2)]),
            lit_list(vec![Value::Int(1)]),
        ],
    ));
    let difference = eval_ok(&intrinsic(
        IntrinsicOp::SetDifference,
        vec![
            lit_list(vec![Value::Int(1), Value::Int(1), Value::Int(2)]),
            lit_list(vec![Value::Int(2)]),
        ],
    ));

    assert_eq!(intersect, list(vec![Value::Int(1), Value::Int(1)]));
    assert_eq!(difference, list(vec![Value::Int(1), Value::Int(1)]));
}

#[test]
fn set_intersect_and_set_difference_handle_empty_operands() {
    let intersect_empty_right = eval_ok(&intrinsic(
        IntrinsicOp::SetIntersect,
        vec![lit_list(vec![Value::Int(1)]), lit_list(vec![])],
    ));
    let intersect_empty_left = eval_ok(&intrinsic(
        IntrinsicOp::SetIntersect,
        vec![lit_list(vec![]), lit_list(vec![Value::Int(1)])],
    ));
    let difference_empty_right = eval_ok(&intrinsic(
        IntrinsicOp::SetDifference,
        vec![lit_list(vec![Value::Int(1)]), lit_list(vec![])],
    ));
    let difference_empty_left = eval_ok(&intrinsic(
        IntrinsicOp::SetDifference,
        vec![lit_list(vec![]), lit_list(vec![Value::Int(1)])],
    ));

    assert_eq!(intersect_empty_right, list(vec![]));
    assert_eq!(intersect_empty_left, list(vec![]));
    assert_eq!(difference_empty_right, list(vec![Value::Int(1)]));
    assert_eq!(difference_empty_left, list(vec![]));
}

#[test]
fn set_is_subset_reports_containment_including_the_empty_set_cases() {
    let is_subset = |a: Vec<Value>, b: Vec<Value>| {
        eval_ok(&intrinsic(
            IntrinsicOp::SetIsSubset,
            vec![lit_list(a), lit_list(b)],
        ))
    };

    assert_eq!(
        is_subset(
            vec![Value::Int(1), Value::Int(2)],
            vec![Value::Int(1), Value::Int(2), Value::Int(3)]
        ),
        Value::Bool(true)
    );
    assert_eq!(
        is_subset(vec![Value::Int(1), Value::Int(4)], vec![Value::Int(1)]),
        Value::Bool(false)
    );
    // The empty set is a subset of everything, including of itself.
    assert_eq!(is_subset(vec![], vec![]), Value::Bool(true));
    assert_eq!(is_subset(vec![], vec![Value::Int(1)]), Value::Bool(true));
    assert_eq!(is_subset(vec![Value::Int(1)], vec![]), Value::Bool(false));
}

#[test]
fn set_operations_reject_non_list_operands_and_wrong_argument_counts() {
    assert_type_error(
        eval_err(&intrinsic(
            IntrinsicOp::SetIntersect,
            vec![lit_int(1), lit_list(vec![])],
        )),
        "list",
    );
    assert_type_error(
        eval_err(&intrinsic(
            IntrinsicOp::SetDifference,
            vec![lit_list(vec![]), lit(map(vec![]))],
        )),
        "list",
    );
    assert_type_error(
        eval_err(&intrinsic(
            IntrinsicOp::SetIsSubset,
            vec![lit_text("ab"), lit_list(vec![])],
        )),
        "list",
    );
    assert_arity_error(
        eval_err(&intrinsic(IntrinsicOp::SetUnion, vec![lit_list(vec![])])),
        "set_union",
        2,
        1,
    );
}

// -- map_has_key / map_size / map_get_default / map_filter_keys --

#[test]
fn map_has_key_distinguishes_an_absent_key_from_one_holding_null() {
    // A `map_get` based check cannot tell these apart, which is the whole
    // reason this operation exists.
    let subject = lit(map(vec![("present", Value::Int(1)), ("null", Value::Null)]));

    let present = eval_ok(&intrinsic(
        IntrinsicOp::MapHasKey,
        vec![subject.clone(), lit_text("present")],
    ));
    let stored_null = eval_ok(&intrinsic(
        IntrinsicOp::MapHasKey,
        vec![subject.clone(), lit_text("null")],
    ));
    let absent = eval_ok(&intrinsic(
        IntrinsicOp::MapHasKey,
        vec![subject, lit_text("missing")],
    ));

    assert_eq!(present, Value::Bool(true));
    assert_eq!(stored_null, Value::Bool(true));
    assert_eq!(absent, Value::Bool(false));
}

#[test]
fn map_has_key_is_false_for_every_key_of_an_empty_map() {
    assert_eq!(
        eval_ok(&intrinsic(
            IntrinsicOp::MapHasKey,
            vec![lit(map(vec![])), lit_text("anything")]
        )),
        Value::Bool(false)
    );
}

#[test]
fn map_has_key_rejects_a_non_map_subject_a_non_text_key_and_a_wrong_argument_count() {
    assert_type_error(
        eval_err(&intrinsic(
            IntrinsicOp::MapHasKey,
            vec![lit_list(vec![]), lit_text("k")],
        )),
        "map",
    );
    assert_type_error(
        eval_err(&intrinsic(
            IntrinsicOp::MapHasKey,
            vec![lit(map(vec![])), lit_int(1)],
        )),
        "text",
    );
    assert_arity_error(
        eval_err(&intrinsic(IntrinsicOp::MapHasKey, vec![lit(map(vec![]))])),
        "map_has_key",
        2,
        1,
    );
}

#[test]
fn map_size_counts_entries_including_those_holding_null() {
    let result = eval_ok(&intrinsic(
        IntrinsicOp::MapSize,
        vec![lit(map(vec![("a", Value::Int(1)), ("b", Value::Null)]))],
    ));

    assert_eq!(result, Value::Int(2));
}

#[test]
fn map_size_of_an_empty_map_is_zero() {
    assert_eq!(
        eval_ok(&intrinsic(IntrinsicOp::MapSize, vec![lit(map(vec![]))])),
        Value::Int(0)
    );
}

#[test]
fn map_size_rejects_a_non_map_argument_and_a_wrong_argument_count() {
    assert_type_error(
        eval_err(&intrinsic(IntrinsicOp::MapSize, vec![lit_list(vec![])])),
        "map",
    );
    assert_arity_error(
        eval_err(&intrinsic(
            IntrinsicOp::MapSize,
            vec![lit(map(vec![])), lit_text("k")],
        )),
        "map_size",
        1,
        2,
    );
}

#[test]
fn map_get_default_returns_the_stored_value_when_the_key_is_present() {
    let result = eval_ok(&intrinsic(
        IntrinsicOp::MapGetDefault,
        vec![
            lit(map(vec![("k", Value::Int(7))])),
            lit_text("k"),
            lit_int(-1),
        ],
    ));

    assert_eq!(result, Value::Int(7));
}

#[test]
fn map_get_default_substitutes_the_default_only_when_the_key_is_absent() {
    // A stored null is a real value, so it wins over the default. Collapsing
    // the two would make a null valued field silently read as the default.
    let stored_null = eval_ok(&intrinsic(
        IntrinsicOp::MapGetDefault,
        vec![
            lit(map(vec![("k", Value::Null)])),
            lit_text("k"),
            lit_int(-1),
        ],
    ));
    let absent = eval_ok(&intrinsic(
        IntrinsicOp::MapGetDefault,
        vec![lit(map(vec![])), lit_text("k"), lit_int(-1)],
    ));

    assert_eq!(stored_null, Value::Null);
    assert_eq!(absent, Value::Int(-1));
}

#[test]
fn map_get_default_accepts_null_as_the_default_itself() {
    let result = eval_ok(&intrinsic(
        IntrinsicOp::MapGetDefault,
        vec![lit(map(vec![])), lit_text("k"), lit(Value::Null)],
    ));

    assert_eq!(result, Value::Null);
}

#[test]
fn map_get_default_rejects_a_non_map_subject_and_a_wrong_argument_count() {
    assert_type_error(
        eval_err(&intrinsic(
            IntrinsicOp::MapGetDefault,
            vec![lit_list(vec![]), lit_text("k"), lit_int(0)],
        )),
        "map",
    );
    assert_arity_error(
        eval_err(&intrinsic(
            IntrinsicOp::MapGetDefault,
            vec![lit(map(vec![])), lit_text("k")],
        )),
        "map_get_default",
        3,
        2,
    );
}

#[test]
fn map_filter_keys_keeps_only_the_listed_keys() {
    let result = eval_ok(&intrinsic(
        IntrinsicOp::MapFilterKeys,
        vec![
            lit(map(vec![
                ("a", Value::Int(1)),
                ("b", Value::Int(2)),
                ("c", Value::Int(3)),
            ])),
            lit_list(vec![text("a"), text("c")]),
        ],
    ));

    assert_eq!(
        result,
        map(vec![("a", Value::Int(1)), ("c", Value::Int(3))])
    );
}

#[test]
fn map_filter_keys_with_an_empty_key_list_or_no_overlap_produces_an_empty_map() {
    let no_keys = eval_ok(&intrinsic(
        IntrinsicOp::MapFilterKeys,
        vec![lit(map(vec![("a", Value::Int(1))])), lit_list(vec![])],
    ));
    let no_overlap = eval_ok(&intrinsic(
        IntrinsicOp::MapFilterKeys,
        vec![
            lit(map(vec![("a", Value::Int(1))])),
            lit_list(vec![text("z")]),
        ],
    ));
    let empty_map = eval_ok(&intrinsic(
        IntrinsicOp::MapFilterKeys,
        vec![lit(map(vec![])), lit_list(vec![text("a")])],
    ));

    assert_eq!(no_keys, map(vec![]));
    assert_eq!(no_overlap, map(vec![]));
    assert_eq!(empty_map, map(vec![]));
}

#[test]
fn map_filter_keys_silently_ignores_non_text_entries_in_the_key_list() {
    // A key list holding an int narrows the result instead of failing, so a
    // procedure that computed its key list wrongly gets a quietly smaller map
    // rather than an error it could recover from.
    let result = eval_ok(&intrinsic(
        IntrinsicOp::MapFilterKeys,
        vec![
            lit(map(vec![("a", Value::Int(1)), ("b", Value::Int(2))])),
            lit_list(vec![text("a"), Value::Int(2)]),
        ],
    ));

    assert_eq!(result, map(vec![("a", Value::Int(1))]));
}

#[test]
fn map_filter_keys_rejects_a_non_map_subject_a_non_list_key_argument_and_a_wrong_argument_count() {
    assert_type_error(
        eval_err(&intrinsic(
            IntrinsicOp::MapFilterKeys,
            vec![lit_list(vec![]), lit_list(vec![])],
        )),
        "map",
    );
    assert_type_error(
        eval_err(&intrinsic(
            IntrinsicOp::MapFilterKeys,
            vec![lit(map(vec![])), lit_text("a")],
        )),
        "list",
    );
    assert_arity_error(
        eval_err(&intrinsic(
            IntrinsicOp::MapFilterKeys,
            vec![lit(map(vec![]))],
        )),
        "map_filter_keys",
        2,
        1,
    );
}
