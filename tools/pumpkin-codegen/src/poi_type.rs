use std::collections::BTreeMap;
use std::fs;

use heck::ToShoutySnakeCase;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use serde::Deserialize;

/// One entry of `point_of_interest_type.json`: vanilla's `PoiType` registry.
#[derive(Deserialize)]
struct PoiTypeJson {
    id: u16,
    max_tickets: u32,
    valid_range: u32,
    states: Vec<u16>,
}

pub fn build() -> TokenStream {
    let content = fs::read_to_string("../../assets/point_of_interest_type.json")
        .expect("read point_of_interest_type.json");
    let types: BTreeMap<String, PoiTypeJson> =
        serde_json::from_str(&content).expect("parse point_of_interest_type.json");

    let mut by_id: Vec<(&String, &PoiTypeJson)> = types.iter().collect();
    by_id.sort_by_key(|(_, json)| json.id);

    let mut consts = Vec::new();
    let mut all = Vec::new();
    let mut from_name_arms = Vec::new();
    let mut state_entries: Vec<(u16, u16)> = Vec::new();
    for (index, (name, json)) in by_id.iter().enumerate() {
        assert_eq!(
            usize::from(json.id),
            index,
            "point of interest type ids are contiguous"
        );
        let bare = name.strip_prefix("minecraft:").unwrap_or(name);
        let ident = format_ident!("{}", bare.to_shouty_snake_case());
        let id = json.id;
        let max_tickets = json.max_tickets;
        let valid_range = json.valid_range;
        let states = &json.states;
        consts.push(quote! {
            pub const #ident: Self = Self {
                id: #id,
                resource_name: #bare,
                max_tickets: #max_tickets,
                valid_range: #valid_range,
                states: &[#(#states),*],
            };
        });
        all.push(quote! { &Self::#ident });
        from_name_arms.push(quote! { #name | #bare => Some(&Self::#ident) });
        state_entries.extend(states.iter().map(|state| (*state, json.id)));
    }
    state_entries.sort_unstable();
    let state_ids = state_entries.iter().map(|(state, _)| *state);
    let state_types = state_entries.iter().map(|(_, id)| *id);

    quote! {
        /* This file is generated. Do not edit manually. */
        use crate::tag::{RegistryKey, Taggable};

        #[doc = "Vanilla `PoiType`: the block states that are a point of interest, how many tickets it holds and how far away it still counts as valid."]
        #[derive(Debug)]
        pub struct PoiType {
            pub id: u16,
            pub resource_name: &'static str,
            #[doc = "How many holders the point admits at once (`PoiType.maxTickets`)."]
            pub max_tickets: u32,
            pub valid_range: u32,
            #[doc = "The block state ids that are this point of interest."]
            pub states: &'static [u16],
        }

        impl PoiType {
            #(#consts)*

            #[doc = "Every type, indexed by its id."]
            pub const ALL: &'static [&'static Self] = &[#(#all),*];

            #[must_use]
            pub fn from_name(name: &str) -> Option<&'static Self> {
                match name {
                    #(#from_name_arms,)*
                    _ => None,
                }
            }

            #[must_use]
            pub fn from_id(id: u16) -> Option<&'static Self> {
                Self::ALL.get(usize::from(id)).copied()
            }

            #[doc = "The point of interest a block state is, if any (vanilla `PoiTypes.forState`)."]
            #[must_use]
            pub fn from_state_id(state_id: u16) -> Option<&'static Self> {
                let index = STATE_IDS.binary_search(&state_id).ok()?;
                Self::from_id(STATE_TYPES[index])
            }

            #[doc = "Vanilla `PoiTypes.hasPoi`."]
            #[must_use]
            pub fn has_poi(state_id: u16) -> bool {
                STATE_IDS.binary_search(&state_id).is_ok()
            }
        }

        static STATE_IDS: &[u16] = &[#(#state_ids),*];
        static STATE_TYPES: &[u16] = &[#(#state_types),*];

        impl PartialEq for PoiType {
            fn eq(&self, other: &Self) -> bool {
                self.id == other.id
            }
        }
        impl Eq for PoiType {}

        impl Taggable for PoiType {
            #[inline]
            fn tag_key() -> RegistryKey {
                RegistryKey::PointOfInterestType
            }
            #[inline]
            fn registry_key(&self) -> &str {
                self.resource_name
            }
            #[inline]
            fn registry_id(&self) -> u16 {
                self.id
            }
        }
    }
}
