//! The households. Each script opens accounts and plays out 24 months of money.

use crate::sim::Sim;

mod family;
mod freelancer;
mod renter;
mod retiree;

pub struct Persona {
    pub key: &'static str,
    pub description: &'static str,
    pub build: fn(u64) -> Sim,
}

pub static ALL: &[Persona] = &[
    Persona {
        key: renter::KEY,
        description: renter::DESCRIPTION,
        build: renter::build,
    },
    Persona {
        key: family::KEY,
        description: family::DESCRIPTION,
        build: family::build,
    },
    Persona {
        key: freelancer::KEY,
        description: freelancer::DESCRIPTION,
        build: freelancer::build,
    },
    Persona {
        key: retiree::KEY,
        description: retiree::DESCRIPTION,
        build: retiree::build,
    },
];
