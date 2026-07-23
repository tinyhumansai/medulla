use std::fs;

use tempfile::tempdir;

use super::*;

#[test]
fn parse_and_append_round_trip_preserves_surrounding_profile() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(PROFILE_FILE);
    let original = "---\nrouting: |\n  Keep this.\n---\n\nWorkspace summary.\n\n## Lessons\n\n## Notes\n\nDo not touch.\n";
    fs::write(&path, original).unwrap();

    assert_eq!(
        add_lesson(
            dir.path(),
            Lesson::new("CI flakes", "rerun the focused test")
        )
        .unwrap(),
        AddLessonOutcome::Added
    );
    assert_eq!(
        list_lessons(dir.path()).unwrap(),
        vec![Lesson::new("CI flakes", "rerun the focused test")]
    );
    let updated = fs::read_to_string(path).unwrap();
    assert!(updated.starts_with("---\nrouting: |\n  Keep this.\n---\n\nWorkspace summary."));
    assert!(updated.contains(
        "## Lessons\n- when CI flakes: rerun the focused test\n\n## Notes\n\nDo not touch."
    ));
}

#[test]
fn exact_re_add_is_idempotent() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(PROFILE_FILE);
    fs::write(&path, "Profile\n\n## Lessons\n\n- when review: run tests\n").unwrap();

    assert_eq!(
        add_lesson(dir.path(), Lesson::new(" review ", " run tests ")).unwrap(),
        AddLessonOutcome::AlreadyPresent
    );
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        "Profile\n\n## Lessons\n\n- when review: run tests\n"
    );
}

#[test]
fn missing_section_is_appended_without_rewriting_profile() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(PROFILE_FILE);
    fs::write(&path, "Profile without trailing newline").unwrap();

    add_lesson(dir.path(), Lesson::new("deploying", "check the region")).unwrap();
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        "Profile without trailing newline\n\n## Lessons\n\n- when deploying: check the region\n"
    );
}

#[test]
fn missing_profile_and_empty_entries_are_typed_errors() {
    let dir = tempdir().unwrap();
    assert!(matches!(
        list_lessons(dir.path()),
        Err(LessonError::MissingProfile(_))
    ));
    fs::write(dir.path().join(PROFILE_FILE), "Profile").unwrap();
    assert!(matches!(
        add_lesson(dir.path(), Lesson::new(" ", "rule")),
        Err(LessonError::EmptyLesson)
    ));
}

#[test]
fn parser_ignores_entries_outside_lessons_and_malformed_lines() {
    let document = "- when outside: ignored\n\n## Lessons\n\n\
        - when trigger: rule: with details\n\
        - something else\n\
        - when missing separator\n\n## Next\n- when later: ignored\n";
    assert_eq!(
        parse_lessons(document),
        vec![Lesson::new("trigger", "rule: with details")]
    );
}

#[test]
fn command_spec_parser_normalizes_and_rejects_bad_shapes() {
    assert_eq!(
        parse_lesson_spec("  tests fail  ->  inspect the first error ").unwrap(),
        Lesson::new("tests fail", "inspect the first error")
    );
    assert!(matches!(
        parse_lesson_spec("no delimiter"),
        Err(LessonError::InvalidLessonFormat)
    ));
    assert!(matches!(
        parse_lesson_spec(" -> rule"),
        Err(LessonError::EmptyLesson)
    ));
}
