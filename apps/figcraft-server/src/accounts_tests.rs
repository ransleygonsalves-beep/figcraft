//! Accounts: an invite makes one account once, sign-in takes the right password only, sessions
//! end when signed out, disabled or the password changes, the last admin stays one, and nothing
//! readable that lets anyone in is written to disk.

use crate::accounts::{Accounts, WRONG};
use crate::test_util::TempDir;

fn accounts() -> (TempDir, Accounts) {
    let dir = TempDir::new("accounts");
    let a = Accounts::load(dir.path()).unwrap();
    (dir, a)
}

#[test]
fn an_invite_makes_one_account_and_signs_it_in() {
    let (dir, mut a) = accounts();
    assert!(a.is_empty());
    let (_, code) = a.create_invite(true).unwrap();
    let signed = a.redeem(&code, " Ada@Example.com ", "Ada  Lovelace", "correct horse").unwrap();
    assert_eq!(signed.user.email, "ada@example.com", "emails are kept in lower case");
    assert_eq!(signed.user.name, "Ada Lovelace");
    assert!(signed.user.admin, "an admin invite makes an admin");
    assert!(a.redeem(&code, "bob@example.com", "Bob", "another pass").is_err(), "an invite works once");
    // The session lets them back in; its token and the password aren't on disk, readable.
    assert_eq!(a.session(&signed.token).map(|(_, u)| u.id), Some(signed.user.id));
    let kept = std::fs::read_to_string(dir.path().join(crate::accounts::ACCOUNTS)).unwrap();
    assert!(!kept.contains(&signed.token) && !kept.contains(&code) && !kept.contains("correct horse"));
    assert!(kept.contains("$argon2id$"), "the password is an Argon2id hash");
    // Kept across a restart.
    let mut again = Accounts::load(dir.path()).unwrap();
    assert_eq!(again.session(&signed.token).map(|(_, u)| u.email), Some("ada@example.com".into()));
}

#[test]
fn sign_in_takes_the_right_password_only() {
    let (_dir, mut a) = accounts();
    let (_, code) = a.create_invite(false).unwrap();
    a.redeem(&code, "bob@example.com", "Bob", "hunter2hunter2").unwrap();
    assert!(a.sign_in("BOB@example.com", "hunter2hunter2").is_ok(), "any case of the email");
    assert_eq!(a.sign_in("bob@example.com", "wrong password").unwrap_err(), WRONG);
    assert_eq!(a.sign_in("nobody@example.com", "hunter2hunter2").unwrap_err(), WRONG, "an unknown email reads the same");
}

#[test]
fn bad_details_are_refused_with_a_reason() {
    let (_dir, mut a) = accounts();
    let (_, code) = a.create_invite(false).unwrap();
    assert!(a.redeem(&code, "not an email", "Bob", "longenough").unwrap_err().contains("email"));
    assert!(a.redeem(&code, "bob@example.com", "Bob", "short").unwrap_err().contains("8 characters"));
    assert!(a.redeem(&code, "bob@example.com", "   ", "longenough").unwrap_err().contains("name"));
    assert!(a.redeem("made-up", "bob@example.com", "Bob", "longenough").is_err());
    // The invite is still good after the refusals.
    a.redeem(&code, "bob@example.com", "Bob", "longenough").unwrap();
    let (_, code2) = a.create_invite(false).unwrap();
    assert!(a.redeem(&code2, "Bob@example.com", "Bob 2", "longenough").unwrap_err().contains("already an account"));
}

#[test]
fn sessions_end_when_signed_out_disabled_or_the_password_changes() {
    let (_dir, mut a) = accounts();
    let (_, admin_code) = a.create_invite(true).unwrap();
    let admin = a.redeem(&admin_code, "admin@example.com", "Admin", "admin password").unwrap();
    let (_, code) = a.create_invite(false).unwrap();
    let one = a.redeem(&code, "dee@example.com", "Dee", "first password").unwrap();
    let two = a.sign_in("dee@example.com", "first password").unwrap();
    a.sign_out(&one.session).unwrap();
    assert!(a.session(&one.token).is_none(), "signed out");
    assert!(a.session(&two.token).is_some(), "the other session goes on");
    // A password change keeps the session that made it and ends the others.
    let three = a.sign_in("dee@example.com", "first password").unwrap();
    assert!(a.change_password(two.user.id, &three.session, "wrong", "second password").is_err());
    a.change_password(two.user.id, &three.session, "first password", "second password").unwrap();
    assert!(a.session(&two.token).is_none() && a.session(&three.token).is_some());
    assert!(a.sign_in("dee@example.com", "second password").is_ok());
    // Disabled: every session ends and signing in is refused.
    a.set_user(admin.user.id, two.user.id, None, Some(true)).unwrap();
    assert!(a.session(&three.token).is_none());
    assert!(a.sign_in("dee@example.com", "second password").unwrap_err().contains("disabled"));
}

#[test]
fn the_last_admin_stays_one_and_admins_dont_change_their_own_account() {
    let (_dir, mut a) = accounts();
    let (_, code) = a.create_invite(true).unwrap();
    let admin = a.redeem(&code, "admin@example.com", "Admin", "admin password").unwrap();
    let (_, code) = a.create_invite(false).unwrap();
    let dee = a.redeem(&code, "dee@example.com", "Dee", "dee password").unwrap();
    assert!(a.set_user(admin.user.id, admin.user.id, Some(false), None).is_err(), "not their own");
    a.set_user(admin.user.id, dee.user.id, Some(true), None).unwrap();
    // Dee, now an admin, can't remove the other admin's role if that would leave none... there
    // are two, so this works; then the last one can't be removed.
    a.set_user(dee.user.id, admin.user.id, Some(false), None).unwrap();
    assert!(a.set_user(admin.user.id, dee.user.id, Some(false), None).unwrap_err().contains("at least one admin"));
    assert!(a.user(dee.user.id).unwrap().admin, "nothing changed");
}

#[test]
fn invites_can_be_withdrawn() {
    let (_dir, mut a) = accounts();
    let (id, code) = a.create_invite(false).unwrap();
    assert_eq!(a.invites().len(), 1);
    assert!(a.invite_open(&code));
    a.revoke_invite(&id).unwrap();
    assert!(!a.invite_open(&code) && a.invites().is_empty());
    assert!(a.redeem(&code, "x@example.com", "X", "longenough").is_err());
}
