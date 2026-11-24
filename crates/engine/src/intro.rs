//! Introductions (ADR-045): Alice introduces Bob to Carol. Carol says yes
//! first and makes a one-time invite for this alone; Alice passes it to Bob;
//! Bob decides whether to add her; Carol's app accepts Bob's request by its
//! code and his key. Nobody passes on a queue or a key of their own.

use super::*;
use crate::data::{Intro, IntroPart};
use termchat_core::proto::INTRO_LEVEL;

/// An introduction not finished by then is dropped, its code too.
const INTRO_TTL: u64 = 7 * 86_400;
/// Introductions waiting for our answer, per introducer.
const MAX_ASKED: usize = 5;

/// What the chat with the introducer asks of us, for the apps.
pub enum PendingIntro {
    /// Alice asks us to meet someone: allow or decline.
    Ask {
        id: u64,
        nickname: String,
        fingerprint: String,
    },
    /// Alice passes on someone's invite: add or ignore.
    Offer {
        id: u64,
        nickname: String,
        fingerprint: String,
    },
}

/// Carol's answer to an introduction.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IntroAnswer {
    Allow,
    /// From a new identity of its own (ADR-014).
    AllowIncognito,
    Decline,
}

fn short(key: &Ed25519Key) -> String {
    termchat_core::fingerprint(key)[..5].to_string()
}

impl Engine {
    /// The nickname contact `i` chose for themself, never our petname for them.
    fn own_nickname(&self, i: usize) -> String {
        let key = self.data.contacts[i].key;
        self.slot_for(&key)
            .and_then(|s| self.core_at(s).nickname(&key).map(str::to_owned))
            .filter(|n| check_nickname(n).is_ok())
            .unwrap_or_else(|| short(&key))
    }

    /// Contact `i` can take part: accepted, reachable, new enough.
    fn introducible(&self, i: usize) -> Result<(), String> {
        let c = self.writable(i)?;
        if c.own {
            return Err("not your own device".into());
        }
        if c.understands < INTRO_LEVEL {
            return Err(format!(
                "{}'s app cannot take introductions yet",
                self.name(c)
            ));
        }
        Ok(())
    }

    /// ADR-045: asks contact `carol` whether they would meet contact `bob`.
    pub fn introduce_contacts(&mut self, bob: usize, carol: usize) -> Result<(), String> {
        if bob == carol {
            return Err("pick someone else to introduce them to".into());
        }
        if !self.is_primary() {
            return Err("introduce people from your main device".into());
        }
        self.introducible(bob)?;
        self.introducible(carol)?;
        let (bob_key, carol_key) = (self.data.contacts[bob].key, self.data.contacts[carol].key);
        let mut id = [0; 8];
        getrandom::fill(&mut id).map_err(|e| e.to_string())?;
        let id = u64::from_le_bytes(id);
        let body = Body::IntroAsk {
            intro: id,
            nickname: self.own_nickname(bob),
            key: bob_key,
        };
        self.transmit(&carol_key, body)?;
        self.data.intros.push(Intro {
            id,
            at: now(),
            part: IntroPart::Asking {
                bob: bob_key,
                carol: carol_key,
            },
        });
        let (bob_name, carol_name) = (
            self.name(&self.data.contacts[bob]),
            self.name(&self.data.contacts[carol]),
        );
        self.system(carol, format!("you asked them to meet {bob_name}"));
        self.system(
            bob,
            format!("asked {carol_name} whether they would meet them"),
        );
        self.persist();
        Ok(())
    }

    /// The introduction contact `i` waits on us for, oldest first.
    pub fn pending_intro(&self, i: usize) -> Option<PendingIntro> {
        let alice_key = self.data.contacts.get(i)?.key;
        self.data.intros.iter().find_map(|intro| match &intro.part {
            IntroPart::Asked {
                alice,
                nickname,
                bob,
            } if *alice == alice_key => Some(PendingIntro::Ask {
                id: intro.id,
                nickname: clean(nickname),
                fingerprint: short(bob),
            }),
            IntroPart::Offered {
                alice,
                nickname,
                invite,
            } if *alice == alice_key => Some(PendingIntro::Offer {
                id: intro.id,
                nickname: clean(nickname),
                fingerprint: proto::Invite::decode(invite)
                    .map(|v| short(&v.identity.ed25519))
                    .unwrap_or_default(),
            }),
            _ => None,
        })
    }

    fn intro_at(&self, id: u64, alice: &Ed25519Key) -> Option<usize> {
        self.data.intros.iter().position(|intro| {
            intro.id == id
                && matches!(&intro.part,
                    IntroPart::Asked { alice: a, .. } | IntroPart::Offered { alice: a, .. } if a == alice)
        })
    }

    /// ADR-045: our answer to contact `i`'s introduction `id`.
    pub fn answer_intro(&mut self, i: usize, id: u64, answer: IntroAnswer) -> Result<(), String> {
        let alice = self.writable(i)?.key;
        let at = self.intro_at(id, &alice).ok_or("no such introduction")?;
        let IntroPart::Asked { nickname, bob, .. } = self.data.intros[at].part.clone() else {
            return Err("no such introduction".into());
        };
        let name = format!("{}·{}", clean(&nickname), short(&bob));
        if answer == IntroAnswer::Decline {
            self.data.intros.remove(at);
            self.transmit(&alice, Body::IntroDecline { intro: id })?;
            self.system(i, format!("you declined to meet {name}"));
            self.persist();
            return Ok(());
        }
        let code = new_code();
        let profile = match answer {
            IntroAnswer::AllowIncognito => {
                let slot = self.new_profile("", Some(code))?;
                Some(self.data.profiles[slot - 1].key)
            }
            _ => None,
        };
        self.data.intros[at].part = IntroPart::Granted {
            alice,
            bob,
            code,
            profile,
            sent: false,
        };
        // The main identity's invite is ready now; an incognito one once its
        // queue exists (`grant_waiting`).
        if profile.is_none() {
            let once = &mut self.data.invites.once;
            once.push(code);
            if once.len() > MAX_ONE_TIME_CODES {
                once.remove(0);
            }
            self.send_grant(at, 0)?;
        }
        self.system(i, format!("you agreed to meet {name}"));
        self.persist();
        Ok(())
    }

    /// Sends Alice the invite for granted introduction `at`, from identity `slot`.
    fn send_grant(&mut self, at: usize, slot: usize) -> Result<(), String> {
        let IntroPart::Granted { alice, code, .. } = self.data.intros[at].part else {
            return Ok(());
        };
        let text = self.invite_with(slot, code)?;
        let invite = proto::Invite::decode(&text).map_err(|e| e.to_string())?;
        let id = self.data.intros[at].id;
        self.transmit(&alice, Body::IntroGrant { intro: id, invite })?;
        if let IntroPart::Granted { sent, .. } = &mut self.data.intros[at].part {
            *sent = true;
        }
        Ok(())
    }

    /// An incognito identity got its invite queue: `true` if an introduction
    /// was waiting for it, and its invite went to the introducer instead of
    /// being shown here.
    pub(super) fn grant_waiting(&mut self, slot: usize) -> bool {
        let Some(key) = self.data.profiles.get(slot.wrapping_sub(1)).map(|p| p.key) else {
            return false;
        };
        let waiting = self.data.intros.iter().position(|intro| {
            matches!(intro.part, IntroPart::Granted { profile: Some(p), sent: false, .. } if p == key)
        });
        let Some(at) = waiting else { return false };
        if let Err(e) = self.send_grant(at, slot) {
            self.note(e);
        }
        self.persist();
        true
    }

    /// ADR-045: as Bob, add the contact introduced in `i`'s chat, or not.
    /// The new contact's index when added.
    pub fn answer_offer(&mut self, i: usize, id: u64, add: bool) -> Result<Option<usize>, String> {
        let alice = self.writable(i)?.key;
        let at = self.intro_at(id, &alice).ok_or("no such introduction")?;
        let IntroPart::Offered {
            nickname, invite, ..
        } = self.data.intros.remove(at).part
        else {
            return Err("no such introduction".into());
        };
        let alice_name = self.name(&self.data.contacts[i]);
        if !add {
            self.system(i, format!("you did not add {}", clean(&nickname)));
            self.persist();
            return Ok(None);
        }
        let j = self.add(&invite, false)?;
        self.system(
            j,
            format!("introduced by {alice_name}; compare your safety number"),
        );
        self.persist();
        Ok(Some(j))
    }

    /// One of the introduction kinds from accepted contact `i`.
    pub(super) fn take_intro(&mut self, i: usize, body: Body) {
        let from = self.data.contacts[i].key;
        let from_name = self.name(&self.data.contacts[i]);
        match body {
            // As Carol.
            Body::IntroAsk {
                intro,
                nickname,
                key,
            } => {
                let asked =
                    |p: &IntroPart| matches!(p, IntroPart::Asked { alice, .. } if *alice == from);
                let waiting = self.data.intros.iter().filter(|x| asked(&x.part)).count();
                if waiting >= MAX_ASKED || self.data.intros.iter().any(|x| x.id == intro) {
                    return;
                }
                let name = format!("{}·{}", clean(&nickname), short(&key));
                if self.slot_of(&key).is_some() || self.index(&key).is_some() {
                    let _ = self.transmit(&from, Body::IntroDecline { intro });
                    self.system(
                        i,
                        format!("they wanted you to meet {name}, whom you already know"),
                    );
                    return;
                }
                self.data.intros.push(Intro {
                    id: intro,
                    at: now(),
                    part: IntroPart::Asked {
                        alice: from,
                        nickname,
                        bob: key,
                    },
                });
                self.system(
                    i,
                    format!("they want to introduce you to {name}: allow or decline"),
                );
                self.happened.push(Happened::Alert {
                    key: from,
                    request: false,
                });
            }
            // As Alice.
            Body::IntroGrant { intro, invite } => {
                let at = self.data.intros.iter().position(|x| {
                    x.id == intro
                        && matches!(x.part, IntroPart::Asking { carol, .. } if carol == from)
                });
                let Some(at) = at else { return };
                let IntroPart::Asking { bob, .. } = self.data.intros.remove(at).part else {
                    return;
                };
                let Some(b) = self.index(&bob) else { return };
                let body = Body::IntroOffer {
                    intro,
                    nickname: self.own_nickname(i),
                    invite,
                };
                let bob_name = self.name(&self.data.contacts[b]);
                match self.transmit(&bob, body) {
                    Ok(_) => {
                        self.system(i, format!("they agreed; {bob_name} got their invite"));
                        self.system(b, format!("introduced them to {from_name}"));
                    }
                    Err(e) => {
                        self.system(b, format!("could not pass on {from_name}'s invite: {e}"))
                    }
                }
            }
            Body::IntroDecline { intro } => {
                let at = self.data.intros.iter().position(|x| {
                    x.id == intro
                        && matches!(x.part, IntroPart::Asking { carol, .. } if carol == from)
                });
                if let Some(at) = at {
                    self.data.intros.remove(at);
                    self.system(i, "they declined the introduction");
                }
            }
            // As Bob.
            Body::IntroOffer {
                intro,
                nickname,
                invite,
            } => {
                let carol = invite.identity.ed25519;
                if self.data.intros.iter().any(|x| x.id == intro)
                    || self.slot_of(&carol).is_some()
                    || self.index(&carol).is_some()
                {
                    return;
                }
                let name = format!("{}·{}", clean(&nickname), short(&carol));
                self.data.intros.push(Intro {
                    id: intro,
                    at: now(),
                    part: IntroPart::Offered {
                        alice: from,
                        nickname,
                        invite: invite.encode(),
                    },
                });
                self.system(i, format!("they introduce {name}: add or ignore"));
                self.happened.push(Happened::Alert {
                    key: from,
                    request: false,
                });
            }
            _ => {}
        }
    }

    /// As Carol: a request from `sender` with `code` that an introduction
    /// granted is accepted at once. `true` if it was.
    pub(super) fn accept_introduced(
        &mut self,
        i: usize,
        sender: &Ed25519Key,
        code: &InviteCode,
    ) -> bool {
        let at = self.data.intros.iter().position(|x| {
            matches!(&x.part, IntroPart::Granted { bob, code: c, .. }
                if bob == sender && bool::from(c.ct_eq(code)))
        });
        let Some(at) = at else { return false };
        let IntroPart::Granted { alice, .. } = self.data.intros.remove(at).part else {
            return false;
        };
        let by = self
            .index(&alice)
            .map(|a| self.name(&self.data.contacts[a]))
            .unwrap_or_default();
        self.accept(i);
        self.system(i, format!("introduced by {by}; compare your safety number"));
        true
    }

    /// Introductions older than `INTRO_TTL` go. `true` if any did.
    pub(super) fn expire_intros(&mut self) -> bool {
        let t = now();
        let before = self.data.intros.len();
        let old: Vec<InviteCode> = self
            .data
            .intros
            .iter()
            .filter(|x| t >= x.at + INTRO_TTL)
            .filter_map(|x| match x.part {
                IntroPart::Granted { code, .. } => Some(code),
                _ => None,
            })
            .collect();
        self.data.invites.once.retain(|c| !old.contains(c));
        self.data.intros.retain(|x| t < x.at + INTRO_TTL);
        self.data.intros.len() != before
    }
}
