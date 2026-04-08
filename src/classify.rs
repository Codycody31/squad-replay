/// Cached classification bitmask for an export group's class name.
///
/// The `is_*` helpers below do naive substring scans. They get called on
/// every property event, but only a few dozen unique class names exist per
/// replay, so we run them once per `ExportGroup` and cache the result here.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClassifyFlags(pub u8);

impl ClassifyFlags {
    pub const SOLDIER: u8 = 1 << 0;
    pub const VEHICLE: u8 = 1 << 1;
    pub const HELICOPTER: u8 = 1 << 2;
    pub const DEPLOYABLE_PRIMARY: u8 = 1 << 3;

    pub fn from_group_leaf(leaf: &str) -> Self {
        let mut bits = 0u8;
        if is_soldier_type(leaf) {
            bits |= Self::SOLDIER;
        }
        if is_helicopter_type(leaf) {
            bits |= Self::HELICOPTER;
        }
        if is_vehicle_type(leaf) {
            bits |= Self::VEHICLE;
        }
        if is_deployable_primary_type(leaf) {
            bits |= Self::DEPLOYABLE_PRIMARY;
        }
        Self(bits)
    }

    #[inline]
    pub fn is_soldier(self) -> bool {
        self.0 & Self::SOLDIER != 0
    }

    #[inline]
    pub fn is_vehicle(self) -> bool {
        self.0 & Self::VEHICLE != 0
    }

    #[inline]
    #[allow(dead_code)]
    pub fn is_helicopter(self) -> bool {
        self.0 & Self::HELICOPTER != 0
    }

    #[inline]
    pub fn is_deployable_primary(self) -> bool {
        self.0 & Self::DEPLOYABLE_PRIMARY != 0
    }
}

fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    needle.is_empty()
        || haystack.len() >= needle.len()
            && haystack
                .windows(needle.len())
                .any(|window| window.eq_ignore_ascii_case(needle))
}

fn starts_with_ignore_ascii_case(haystack: &str, prefix: &str) -> bool {
    let haystack = haystack.as_bytes();
    let prefix = prefix.as_bytes();
    haystack.len() >= prefix.len() && haystack[..prefix.len()].eq_ignore_ascii_case(prefix)
}

pub fn normalize_type(type_name: &str) -> Option<String> {
    if type_name.is_empty() {
        return None;
    }
    if let Some(idx) = type_name.rfind('.') {
        return Some(type_name[idx + 1..].to_string());
    }
    Some(type_name.to_string())
}

pub fn is_soldier_type(type_name: &str) -> bool {
    contains_ignore_ascii_case(type_name, "soldier")
}

pub fn is_helicopter_type(type_name: &str) -> bool {
    [
        "loach",
        "uh1",
        "uh-1",
        "uh60",
        "uh-60",
        "blackhawk",
        "black hawk",
        "mi8",
        "mi-8",
        "mi17",
        "mi-17",
        "ch146",
        "ch-146",
        "ch178",
        "ch-178",
        "griffon",
        "raven",
        "mrh90",
        "mrh-90",
        "sa330",
        "sa-330",
        "puma",
        "z8",
        "z-8",
        "z9",
        "z-9",
        "helicopter",
        "heli",
    ]
    .iter()
    .any(|needle| contains_ignore_ascii_case(type_name, needle))
}

pub fn is_deployable_primary_type(type_name: &str) -> bool {
    if contains_ignore_ascii_case(type_name, "sqdeployablechildactor_gen_variable") {
        return false;
    }
    if contains_ignore_ascii_case(type_name, "weapon")
        || contains_ignore_ascii_case(type_name, "baseplate")
        || contains_ignore_ascii_case(type_name, "repairtool")
    {
        return false;
    }
    if starts_with_ignore_ascii_case(type_name, "bp_emplaced") {
        return false;
    }
    contains_ignore_ascii_case(type_name, "fobradio")
        || contains_ignore_ascii_case(type_name, "_hab_")
        || contains_ignore_ascii_case(type_name, "hab_")
        || contains_ignore_ascii_case(type_name, "ammocrate")
        || contains_ignore_ascii_case(type_name, "vehicle_repair")
        || contains_ignore_ascii_case(type_name, "rallypoint")
        || contains_ignore_ascii_case(type_name, "_deployable")
        || contains_ignore_ascii_case(type_name, "_tripod")
        || contains_ignore_ascii_case(type_name, "dshk")
        || contains_ignore_ascii_case(type_name, "kord_tripod")
        || contains_ignore_ascii_case(type_name, "kornet_tripod")
        || contains_ignore_ascii_case(type_name, "spg9_tripod")
        || contains_ignore_ascii_case(type_name, "hj-8atgm_deployable")
        || contains_ignore_ascii_case(type_name, "hj-8atgm_tripod")
        || contains_ignore_ascii_case(type_name, "mk19_tripod")
        || contains_ignore_ascii_case(type_name, "zu-23_emplacement")
}

pub fn is_vehicle_type(type_name: &str) -> bool {
    if is_soldier_type(type_name) || is_deployable_primary_type(type_name) {
        return false;
    }
    for needle in [
        "seat",
        "turret",
        "passenger",
        "weapon",
        "ammowep",
        "smokegenerator",
        "resourceweapon",
        "projectile",
        "commander",
        "cupola",
        "doorgun",
        "doorgun",
        "launcher",
        "destruction",
        "turret1",
        "turret2",
        "turret3",
        "cmdr",
        "pintle",
        "commander_turret",
    ] {
        if contains_ignore_ascii_case(type_name, needle) {
            return false;
        }
    }
    if is_helicopter_type(type_name) {
        return true;
    }
    for needle in [
        "m1151",
        "matv",
        "m1a1",
        "t72",
        "t62",
        "m60",
        "brdm",
        "lav",
        "btr",
        "aavp",
        "quadbike",
        "sprut",
        "bmp",
        "ural",
        "m939",
        "safir",
        "uh1",
        "mi17",
        "truck",
        "technical",
        "mtlb",
        "humvee",
        "tank",
        "logistics",
        "heli",
        "loach",
    ] {
        if contains_ignore_ascii_case(type_name, needle) {
            return true;
        }
    }
    false
}

pub fn classify_deployable_event_type(type_name: &str) -> &'static str {
    if contains_ignore_ascii_case(type_name, "fobradio") {
        "RADIO"
    } else if contains_ignore_ascii_case(type_name, "_hab_")
        || contains_ignore_ascii_case(type_name, "hab_")
    {
        "HAB"
    } else if contains_ignore_ascii_case(type_name, "rallypoint") {
        "RALLY"
    } else if contains_ignore_ascii_case(type_name, "ammocrate") {
        "AMMO"
    } else if contains_ignore_ascii_case(type_name, "vehicle_repair") {
        "REPAIR"
    } else if contains_ignore_ascii_case(type_name, "mortar") {
        "MORTAR"
    } else if [
        "tripod",
        "dshk",
        "kord",
        "kornet",
        "spg9",
        "tow",
        "hj-8",
        "mk19",
        "zu-23",
        "emplacement",
    ]
    .iter()
    .any(|needle| contains_ignore_ascii_case(type_name, needle))
    {
        "EMPLACEMENT"
    } else {
        "DEPLOYABLE"
    }
}

pub fn infer_component_type_name(group_path: &str, path_hint: Option<&str>) -> &'static str {
    if contains_ignore_ascii_case(group_path, "sqrotorcomponent")
        || path_hint.is_some_and(|hint| contains_ignore_ascii_case(hint, "rotor"))
    {
        "rotor"
    } else if contains_ignore_ascii_case(group_path, "sqvehicletrack")
        || path_hint.is_some_and(|hint| contains_ignore_ascii_case(hint, "track"))
    {
        "track"
    } else if contains_ignore_ascii_case(group_path, "sqvehicleammobox")
        || path_hint.is_some_and(|hint| contains_ignore_ascii_case(hint, "ammorack"))
    {
        "ammorack"
    } else if contains_ignore_ascii_case(group_path, "sqvehiclewheel")
        || path_hint.is_some_and(|hint| contains_ignore_ascii_case(hint, "wheel"))
    {
        "wheel"
    } else if contains_ignore_ascii_case(group_path, "sqvehicleseatcomponent")
        || path_hint.is_some_and(|hint| contains_ignore_ascii_case(hint, "seat"))
    {
        "seat"
    } else {
        "component"
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn infer_component_type(group_path: &str, path_hint: Option<&str>) -> String {
    infer_component_type_name(group_path, path_hint).to_string()
}

pub fn infer_group_leaf(path: &str) -> &str {
    if let Some(idx) = path.rfind('.') {
        return &path[idx + 1..];
    }
    if let Some(idx) = path.rfind('/') {
        return &path[idx + 1..];
    }
    path
}

#[cfg(test)]
mod tests {
    use super::{infer_component_type, is_helicopter_type, is_vehicle_type};

    #[test]
    fn vehicle_seat_components_are_classified_as_seats() {
        assert_eq!(
            infer_component_type("/Script/Squad.SQVehicleSeatComponent", None),
            "seat"
        );
    }

    #[test]
    fn helicopter_classification_covers_current_families() {
        for type_name in [
            "BP_UH60M_C",
            "BP_CH146_Utility_C",
            "BP_CH178_Transport_C",
            "BP_Mi8MTV5_C",
            "BP_MRH90_C",
            "BP_SA330_C",
            "BP_Z8G_C",
            "BP_Z9A_C",
        ] {
            assert!(
                is_helicopter_type(type_name),
                "{type_name} should be a helicopter"
            );
            assert!(
                is_vehicle_type(type_name),
                "{type_name} should be a vehicle"
            );
        }
    }
}
