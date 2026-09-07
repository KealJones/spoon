//! Text rendering for `spoon doctor`.

use super::{BucketReport, DoctorReport, ExampleRow};

pub fn print_report(r: &DoctorReport) {
    // Header: build identity and scope
    if r.all_mode {
        println!("reporting on all builds; {} episodes total", r.total);
        if r.unstamped_count > 0 {
            println!(
                "  {} episode(s) have no build stamp (pre-dating this field); \
                 shown as 'unknown build' below",
                r.unstamped_count
            );
        }
    } else if r.total == 0 {
        println!(
            "build {}  (no episodes yet - run some turns first)",
            r.build_id
        );
        return;
    } else {
        let excl = if r.excluded_count > 0 {
            format!("; {} older episode(s) excluded, use --all", r.excluded_count)
        } else {
            String::new()
        };
        println!("reporting on build {}{}", r.build_id, excl);
    }

    println!(
        "{} episode(s), {} unsatisfying ({:.0}%)\n",
        r.total, r.failures, r.failure_rate_pct
    );

    let w = &r.weaning;
    println!(
        "weaning: ears_native={} ears_llm={} ears_failed={} mouth_llm={} teacher_llm={}",
        w.ears_native, w.ears_llm, w.ears_failed, w.mouth_llm, w.teacher_llm
    );

    if r.interior_llm_violation {
        eprintln!(
            "\n*** INTERIOR LLM VIOLATION: {} interior calls detected ***",
            w.interior_llm_calls
        );
    } else {
        println!("interior_llm_calls: 0 (ok)");
    }

    if r.honest_unknowns > 0 {
        println!(
            "honest unknowns: {} (not counted as failures)",
            r.honest_unknowns
        );
    }
    if r.slow > 0 {
        println!("slow (>3 s): {}", r.slow);
    }

    if r.buckets.is_empty() {
        println!("\nno failures found");
        return;
    }

    println!();
    for bucket in &r.buckets {
        print_bucket(bucket, &r.build_id, r.all_mode);
    }

    if r.all_mode && r.unstamped_count > 0 {
        println!("(unknown build)  {} episode(s) with no build stamp", r.unstamped_count);
        println!();
    }
}

fn print_bucket(bucket: &BucketReport, current_build: &str, all_mode: bool) {
    println!("{}  ({})", bucket_label(&bucket.bucket), bucket.count);
    for cluster in &bucket.clusters {
        let count_str = if all_mode && cluster.current_build_count < cluster.count {
            let older = cluster.count - cluster.current_build_count;
            if cluster.current_build_count > 0 {
                format!(
                    "[{}x current ({}), {}x older]",
                    cluster.current_build_count, current_build, older
                )
            } else {
                format!("[{}x older builds only]", older)
            }
        } else {
            format!("[{}x]", cluster.count)
        };
        println!("   {} {}", count_str, symptom_label(&cluster.symptom));
        println!("         {}", cluster.suggests);
        for ex in &cluster.examples {
            print_example(ex, all_mode);
        }
    }
    println!();
}

fn print_example(ex: &ExampleRow, all_mode: bool) {
    let det = if ex.details.is_empty() {
        String::new()
    } else {
        format!(" [{}]", ex.details)
    };
    let sec = if ex.secondary_symptoms.is_empty() {
        String::new()
    } else {
        format!(" (also: {})", ex.secondary_symptoms.join(", "))
    };
    // Only annotate with [unknown build] in --all mode when the example has no stamp.
    let build_note = if all_mode && ex.build_id.is_empty() {
        " [unknown build]".to_string()
    } else {
        String::new()
    };
    if ex.sce.is_empty() {
        println!("      '{}'  [{}]{}{}{}", ex.user_text, ex.shape, det, sec, build_note);
    } else {
        println!(
            "      '{}'\n         -> '{}'  [{}]{}{}{}",
            ex.user_text, ex.sce, ex.shape, det, sec, build_note
        );
    }
}

pub fn bucket_label(bucket: &str) -> &str {
    match bucket {
        "ears_no_parse" => "ears: no parse at all",
        "ears_didnt_reparse" => "ears: LLM output did not re-parse",
        "asked_to_rephrase" => "asked to rephrase",
        "runtime_error" => "runtime error",
        "synthesis_failed" => "synthesis failed",
        _ => bucket,
    }
}

pub fn symptom_label(s: &str) -> &str {
    match s {
        "normalizer_inversion" => "normalizer inversion",
        "typo_repair_damage" => "typo repair damage",
        "discourse_lead_in" => "discourse lead-in retained",
        "no_llm_output" => "no LLM output",
        "unparsed" => "unparsed (no specific symptom)",
        _ => s,
    }
}

/// One line per cluster explaining what to fix.
pub fn symptom_suggests(symptom: &str, count: usize, total: usize) -> String {
    match symptom {
        "normalizer_inversion" => {
            "normalizer inverts subject/object; review the normalization prompt template".into()
        }
        "typo_repair_damage" => {
            // The word was changed by local typo repair in lexicon.rs against
            // data/seed/common_words.txt (the file has ~1714 words; its header
            // claims ~2500). Add the word to common_words.txt to protect it.
            "word rewritten by local typo repair (lexicon.rs + data/seed/common_words.txt, \
             ~1714 words vs ~2500 claimed); add the affected word(s) to common_words.txt \
             to protect them from repair"
                .into()
        }
        "discourse_lead_in" => {
            "filler prefix survived into the SCE; strip discourse markers before parsing".into()
        }
        "no_llm_output" => {
            "LLM produced no parseable output; these may need normalizer prompt tuning".into()
        }
        "unparsed" => {
            let pct = count * 100 / total.max(1);
            if pct > 50 {
                format!(
                    "fallback holds {pct}% of failures - \
                     the symptom detectors are not earning their keep; \
                     add more specific detectors"
                )
            } else {
                "no specific symptom detected; inspect manually".into()
            }
        }
        _ => String::new(),
    }
}

// ---- input shape (per-example detail) ------------------------------------

pub fn input_shape(text: &str) -> String {
    let tokens: Vec<String> = text.split_whitespace().map(substitute_token).collect();

    let mut i = 0;
    let mut parts: Vec<String> = Vec::new();

    while i < tokens.len() {
        let t = strip_punct(&tokens[i]);
        if super::detect::is_function_word(t) {
            parts.push(t.to_string());
            i += 1;
        } else {
            break;
        }
    }

    if i < tokens.len() {
        let t = strip_punct(&tokens[i]);
        let first = if t.starts_with('<') { t.to_string() } else { "<verb>".to_string() };
        parts.push(first);
        i += 1;

        let rest: Vec<String> = tokens[i..]
            .iter()
            .filter_map(|tok| {
                let c = strip_punct(tok);
                c.starts_with('<').then(|| c.to_string())
            })
            .collect();

        if !rest.is_empty() {
            parts.push("...".to_string());
            parts.extend(rest);
        }
    }

    if parts.is_empty() {
        tokens.into_iter().take(4).collect::<Vec<_>>().join(" ")
    } else {
        parts.join(" ")
    }
}

fn substitute_token(token: &str) -> String {
    let t = token.trim_end_matches(|c: char| matches!(c, '.' | ',' | '!' | '?' | ';' | ':'));
    if t.starts_with("http://") || t.starts_with("https://") {
        return "<url>".to_string();
    }
    if (t.starts_with('/') || t.starts_with("./") || t.starts_with("../"))
        && t.len() > 1
        && !t.starts_with("//")
    {
        return "<path>".to_string();
    }
    if t.starts_with('"') || t.starts_with('\'') || t.starts_with('`') {
        return "<quoted>".to_string();
    }
    if !t.is_empty()
        && t.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)
        && t.chars().all(|c| c.is_ascii_digit() || c == '.')
    {
        return "<n>".to_string();
    }
    token.to_lowercase()
}

fn strip_punct(s: &str) -> &str {
    s.trim_matches(|c: char| !c.is_alphanumeric() && c != '<' && c != '>' && c != '_')
}
