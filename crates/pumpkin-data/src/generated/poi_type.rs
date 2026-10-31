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
    pub const ARMORER: Self = Self {
        id: 0u16,
        resource_name: "armorer",
        max_tickets: 1u32,
        valid_range: 1u32,
        states: &[
            24119u16, 24120u16, 24121u16, 24122u16, 24123u16, 24124u16, 24125u16, 24126u16,
        ],
    };
    pub const BUTCHER: Self = Self {
        id: 1u16,
        resource_name: "butcher",
        max_tickets: 1u32,
        valid_range: 1u32,
        states: &[
            24111u16, 24112u16, 24113u16, 24114u16, 24115u16, 24116u16, 24117u16, 24118u16,
        ],
    };
    pub const CARTOGRAPHER: Self = Self {
        id: 2u16,
        resource_name: "cartographer",
        max_tickets: 1u32,
        valid_range: 1u32,
        states: &[24127u16],
    };
    pub const CLERIC: Self = Self {
        id: 3u16,
        resource_name: "cleric",
        max_tickets: 1u32,
        valid_range: 1u32,
        states: &[
            11186u16, 11187u16, 11188u16, 11189u16, 11190u16, 11191u16, 11192u16, 11193u16,
        ],
    };
    pub const FARMER: Self = Self {
        id: 4u16,
        resource_name: "farmer",
        max_tickets: 1u32,
        valid_range: 1u32,
        states: &[
            25100u16, 25101u16, 25102u16, 25103u16, 25104u16, 25105u16, 25106u16, 25107u16,
            25108u16,
        ],
    };
    pub const FISHERMAN: Self = Self {
        id: 5u16,
        resource_name: "fisherman",
        max_tickets: 1u32,
        valid_range: 1u32,
        states: &[
            24099u16, 24100u16, 24101u16, 24102u16, 24103u16, 24104u16, 24105u16, 24106u16,
            24107u16, 24108u16, 24109u16, 24110u16,
        ],
    };
    pub const FLETCHER: Self = Self {
        id: 6u16,
        resource_name: "fletcher",
        max_tickets: 1u32,
        valid_range: 1u32,
        states: &[24128u16],
    };
    pub const LEATHERWORKER: Self = Self {
        id: 7u16,
        resource_name: "leatherworker",
        max_tickets: 1u32,
        valid_range: 1u32,
        states: &[
            11194u16, 11195u16, 11196u16, 11197u16, 11198u16, 11199u16, 11200u16, 11201u16,
        ],
    };
    pub const LIBRARIAN: Self = Self {
        id: 8u16,
        resource_name: "librarian",
        max_tickets: 1u32,
        valid_range: 1u32,
        states: &[
            24141u16, 24142u16, 24143u16, 24144u16, 24145u16, 24146u16, 24147u16, 24148u16,
            24149u16, 24150u16, 24151u16, 24152u16, 24153u16, 24154u16, 24155u16, 24156u16,
        ],
    };
    pub const MASON: Self = Self {
        id: 9u16,
        resource_name: "mason",
        max_tickets: 1u32,
        valid_range: 1u32,
        states: &[24158u16, 24159u16, 24160u16, 24161u16],
    };
    pub const SHEPHERD: Self = Self {
        id: 10u16,
        resource_name: "shepherd",
        max_tickets: 1u32,
        valid_range: 1u32,
        states: &[24095u16, 24096u16, 24097u16, 24098u16],
    };
    pub const TOOLSMITH: Self = Self {
        id: 11u16,
        resource_name: "toolsmith",
        max_tickets: 1u32,
        valid_range: 1u32,
        states: &[24157u16],
    };
    pub const WEAPONSMITH: Self = Self {
        id: 12u16,
        resource_name: "weaponsmith",
        max_tickets: 1u32,
        valid_range: 1u32,
        states: &[
            24129u16, 24130u16, 24131u16, 24132u16, 24133u16, 24134u16, 24135u16, 24136u16,
            24137u16, 24138u16, 24139u16, 24140u16,
        ],
    };
    pub const HOME: Self = Self {
        id: 13u16,
        resource_name: "home",
        max_tickets: 1u32,
        valid_range: 1u32,
        states: &[
            2030u16, 2032u16, 2034u16, 2036u16, 2038u16, 2040u16, 2042u16, 2044u16, 2046u16,
            2048u16, 2050u16, 2052u16, 2054u16, 2056u16, 2058u16, 2060u16, 2062u16, 2064u16,
            2066u16, 2068u16, 2070u16, 2072u16, 2074u16, 2076u16, 2078u16, 2080u16, 2082u16,
            2084u16, 2086u16, 2088u16, 2090u16, 2092u16, 2094u16, 2096u16, 2098u16, 2100u16,
            2102u16, 2104u16, 2106u16, 2108u16, 2110u16, 2112u16, 2114u16, 2116u16, 2118u16,
            2120u16, 2122u16, 2124u16, 2126u16, 2128u16, 2130u16, 2132u16, 2134u16, 2136u16,
            2138u16, 2140u16, 2142u16, 2144u16, 2146u16, 2148u16, 2150u16, 2152u16, 2154u16,
            2156u16, 2158u16, 2160u16, 2162u16, 2164u16, 2166u16, 2168u16, 2170u16, 2172u16,
            2174u16, 2176u16, 2178u16, 2180u16, 2182u16, 2184u16, 2186u16, 2188u16, 2190u16,
            2192u16, 2194u16, 2196u16, 2198u16, 2200u16, 2202u16, 2204u16, 2206u16, 2208u16,
            2210u16, 2212u16, 2214u16, 2216u16, 2218u16, 2220u16, 2222u16, 2224u16, 2226u16,
            2228u16, 2230u16, 2232u16, 2234u16, 2236u16, 2238u16, 2240u16, 2242u16, 2244u16,
            2246u16, 2248u16, 2250u16, 2252u16, 2254u16, 2256u16, 2258u16, 2260u16, 2262u16,
            2264u16, 2266u16, 2268u16, 2270u16, 2272u16, 2274u16, 2276u16, 2278u16, 2280u16,
            2282u16, 2284u16,
        ],
    };
    pub const MEETING: Self = Self {
        id: 14u16,
        resource_name: "meeting",
        max_tickets: 32u32,
        valid_range: 6u32,
        states: &[
            24162u16, 24163u16, 24164u16, 24165u16, 24166u16, 24167u16, 24168u16, 24169u16,
            24170u16, 24171u16, 24172u16, 24173u16, 24174u16, 24175u16, 24176u16, 24177u16,
            24178u16, 24179u16, 24180u16, 24181u16, 24182u16, 24183u16, 24184u16, 24185u16,
            24186u16, 24187u16, 24188u16, 24189u16, 24190u16, 24191u16, 24192u16, 24193u16,
        ],
    };
    pub const BEEHIVE: Self = Self {
        id: 15u16,
        resource_name: "beehive",
        max_tickets: 0u32,
        valid_range: 1u32,
        states: &[
            25149u16, 25150u16, 25151u16, 25152u16, 25153u16, 25154u16, 25155u16, 25156u16,
            25157u16, 25158u16, 25159u16, 25160u16, 25161u16, 25162u16, 25163u16, 25164u16,
            25165u16, 25166u16, 25167u16, 25168u16, 25169u16, 25170u16, 25171u16, 25172u16,
        ],
    };
    pub const BEE_NEST: Self = Self {
        id: 16u16,
        resource_name: "bee_nest",
        max_tickets: 0u32,
        valid_range: 1u32,
        states: &[
            25125u16, 25126u16, 25127u16, 25128u16, 25129u16, 25130u16, 25131u16, 25132u16,
            25133u16, 25134u16, 25135u16, 25136u16, 25137u16, 25138u16, 25139u16, 25140u16,
            25141u16, 25142u16, 25143u16, 25144u16, 25145u16, 25146u16, 25147u16, 25148u16,
        ],
    };
    pub const NETHER_PORTAL: Self = Self {
        id: 17u16,
        resource_name: "nether_portal",
        max_tickets: 0u32,
        valid_range: 1u32,
        states: &[8687u16, 8688u16],
    };
    pub const LODESTONE: Self = Self {
        id: 18u16,
        resource_name: "lodestone",
        max_tickets: 0u32,
        valid_range: 1u32,
        states: &[25187u16],
    };
    pub const TEST_INSTANCE: Self = Self {
        id: 19u16,
        resource_name: "test_instance",
        max_tickets: 0u32,
        valid_range: 1u32,
        states: &[25099u16],
    };
    pub const LIGHTNING_ROD: Self = Self {
        id: 20u16,
        resource_name: "lightning_rod",
        max_tickets: 0u32,
        valid_range: 1u32,
        states: &[
            33373u16, 33374u16, 33375u16, 33376u16, 33377u16, 33378u16, 33379u16, 33380u16,
            33381u16, 33382u16, 33383u16, 33384u16, 33385u16, 33386u16, 33387u16, 33388u16,
            33389u16, 33390u16, 33391u16, 33392u16, 33393u16, 33394u16, 33395u16, 33396u16,
            33397u16, 33398u16, 33399u16, 33400u16, 33401u16, 33402u16, 33403u16, 33404u16,
            33405u16, 33406u16, 33407u16, 33408u16, 33409u16, 33410u16, 33411u16, 33412u16,
            33413u16, 33414u16, 33415u16, 33416u16, 33417u16, 33418u16, 33419u16, 33420u16,
            33421u16, 33422u16, 33423u16, 33424u16, 33425u16, 33426u16, 33427u16, 33428u16,
            33429u16, 33430u16, 33431u16, 33432u16, 33433u16, 33434u16, 33435u16, 33436u16,
            33437u16, 33438u16, 33439u16, 33440u16, 33441u16, 33442u16, 33443u16, 33444u16,
            33445u16, 33446u16, 33447u16, 33448u16, 33449u16, 33450u16, 33451u16, 33452u16,
            33453u16, 33454u16, 33455u16, 33456u16, 33457u16, 33458u16, 33459u16, 33460u16,
            33461u16, 33462u16, 33463u16, 33464u16, 33465u16, 33466u16, 33467u16, 33468u16,
            33469u16, 33470u16, 33471u16, 33472u16, 33473u16, 33474u16, 33475u16, 33476u16,
            33477u16, 33478u16, 33479u16, 33480u16, 33481u16, 33482u16, 33483u16, 33484u16,
            33485u16, 33486u16, 33487u16, 33488u16, 33489u16, 33490u16, 33491u16, 33492u16,
            33493u16, 33494u16, 33495u16, 33496u16, 33497u16, 33498u16, 33499u16, 33500u16,
            33501u16, 33502u16, 33503u16, 33504u16, 33505u16, 33506u16, 33507u16, 33508u16,
            33509u16, 33510u16, 33511u16, 33512u16, 33513u16, 33514u16, 33515u16, 33516u16,
            33517u16, 33518u16, 33519u16, 33520u16, 33521u16, 33522u16, 33523u16, 33524u16,
            33525u16, 33526u16, 33527u16, 33528u16, 33529u16, 33530u16, 33531u16, 33532u16,
            33533u16, 33534u16, 33535u16, 33536u16, 33537u16, 33538u16, 33539u16, 33540u16,
            33541u16, 33542u16, 33543u16, 33544u16, 33545u16, 33546u16, 33547u16, 33548u16,
            33549u16, 33550u16, 33551u16, 33552u16, 33553u16, 33554u16, 33555u16, 33556u16,
            33557u16, 33558u16, 33559u16, 33560u16, 33561u16, 33562u16, 33563u16, 33564u16,
        ],
    };
    #[doc = "Every type, indexed by its id."]
    pub const ALL: &'static [&'static Self] = &[
        &Self::ARMORER,
        &Self::BUTCHER,
        &Self::CARTOGRAPHER,
        &Self::CLERIC,
        &Self::FARMER,
        &Self::FISHERMAN,
        &Self::FLETCHER,
        &Self::LEATHERWORKER,
        &Self::LIBRARIAN,
        &Self::MASON,
        &Self::SHEPHERD,
        &Self::TOOLSMITH,
        &Self::WEAPONSMITH,
        &Self::HOME,
        &Self::MEETING,
        &Self::BEEHIVE,
        &Self::BEE_NEST,
        &Self::NETHER_PORTAL,
        &Self::LODESTONE,
        &Self::TEST_INSTANCE,
        &Self::LIGHTNING_ROD,
    ];
    #[must_use]
    pub fn from_name(name: &str) -> Option<&'static Self> {
        match name {
            "minecraft:armorer" | "armorer" => Some(&Self::ARMORER),
            "minecraft:butcher" | "butcher" => Some(&Self::BUTCHER),
            "minecraft:cartographer" | "cartographer" => Some(&Self::CARTOGRAPHER),
            "minecraft:cleric" | "cleric" => Some(&Self::CLERIC),
            "minecraft:farmer" | "farmer" => Some(&Self::FARMER),
            "minecraft:fisherman" | "fisherman" => Some(&Self::FISHERMAN),
            "minecraft:fletcher" | "fletcher" => Some(&Self::FLETCHER),
            "minecraft:leatherworker" | "leatherworker" => Some(&Self::LEATHERWORKER),
            "minecraft:librarian" | "librarian" => Some(&Self::LIBRARIAN),
            "minecraft:mason" | "mason" => Some(&Self::MASON),
            "minecraft:shepherd" | "shepherd" => Some(&Self::SHEPHERD),
            "minecraft:toolsmith" | "toolsmith" => Some(&Self::TOOLSMITH),
            "minecraft:weaponsmith" | "weaponsmith" => Some(&Self::WEAPONSMITH),
            "minecraft:home" | "home" => Some(&Self::HOME),
            "minecraft:meeting" | "meeting" => Some(&Self::MEETING),
            "minecraft:beehive" | "beehive" => Some(&Self::BEEHIVE),
            "minecraft:bee_nest" | "bee_nest" => Some(&Self::BEE_NEST),
            "minecraft:nether_portal" | "nether_portal" => Some(&Self::NETHER_PORTAL),
            "minecraft:lodestone" | "lodestone" => Some(&Self::LODESTONE),
            "minecraft:test_instance" | "test_instance" => Some(&Self::TEST_INSTANCE),
            "minecraft:lightning_rod" | "lightning_rod" => Some(&Self::LIGHTNING_ROD),
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
static STATE_IDS: &[u16] = &[
    2030u16, 2032u16, 2034u16, 2036u16, 2038u16, 2040u16, 2042u16, 2044u16, 2046u16, 2048u16,
    2050u16, 2052u16, 2054u16, 2056u16, 2058u16, 2060u16, 2062u16, 2064u16, 2066u16, 2068u16,
    2070u16, 2072u16, 2074u16, 2076u16, 2078u16, 2080u16, 2082u16, 2084u16, 2086u16, 2088u16,
    2090u16, 2092u16, 2094u16, 2096u16, 2098u16, 2100u16, 2102u16, 2104u16, 2106u16, 2108u16,
    2110u16, 2112u16, 2114u16, 2116u16, 2118u16, 2120u16, 2122u16, 2124u16, 2126u16, 2128u16,
    2130u16, 2132u16, 2134u16, 2136u16, 2138u16, 2140u16, 2142u16, 2144u16, 2146u16, 2148u16,
    2150u16, 2152u16, 2154u16, 2156u16, 2158u16, 2160u16, 2162u16, 2164u16, 2166u16, 2168u16,
    2170u16, 2172u16, 2174u16, 2176u16, 2178u16, 2180u16, 2182u16, 2184u16, 2186u16, 2188u16,
    2190u16, 2192u16, 2194u16, 2196u16, 2198u16, 2200u16, 2202u16, 2204u16, 2206u16, 2208u16,
    2210u16, 2212u16, 2214u16, 2216u16, 2218u16, 2220u16, 2222u16, 2224u16, 2226u16, 2228u16,
    2230u16, 2232u16, 2234u16, 2236u16, 2238u16, 2240u16, 2242u16, 2244u16, 2246u16, 2248u16,
    2250u16, 2252u16, 2254u16, 2256u16, 2258u16, 2260u16, 2262u16, 2264u16, 2266u16, 2268u16,
    2270u16, 2272u16, 2274u16, 2276u16, 2278u16, 2280u16, 2282u16, 2284u16, 8687u16, 8688u16,
    11186u16, 11187u16, 11188u16, 11189u16, 11190u16, 11191u16, 11192u16, 11193u16, 11194u16,
    11195u16, 11196u16, 11197u16, 11198u16, 11199u16, 11200u16, 11201u16, 24095u16, 24096u16,
    24097u16, 24098u16, 24099u16, 24100u16, 24101u16, 24102u16, 24103u16, 24104u16, 24105u16,
    24106u16, 24107u16, 24108u16, 24109u16, 24110u16, 24111u16, 24112u16, 24113u16, 24114u16,
    24115u16, 24116u16, 24117u16, 24118u16, 24119u16, 24120u16, 24121u16, 24122u16, 24123u16,
    24124u16, 24125u16, 24126u16, 24127u16, 24128u16, 24129u16, 24130u16, 24131u16, 24132u16,
    24133u16, 24134u16, 24135u16, 24136u16, 24137u16, 24138u16, 24139u16, 24140u16, 24141u16,
    24142u16, 24143u16, 24144u16, 24145u16, 24146u16, 24147u16, 24148u16, 24149u16, 24150u16,
    24151u16, 24152u16, 24153u16, 24154u16, 24155u16, 24156u16, 24157u16, 24158u16, 24159u16,
    24160u16, 24161u16, 24162u16, 24163u16, 24164u16, 24165u16, 24166u16, 24167u16, 24168u16,
    24169u16, 24170u16, 24171u16, 24172u16, 24173u16, 24174u16, 24175u16, 24176u16, 24177u16,
    24178u16, 24179u16, 24180u16, 24181u16, 24182u16, 24183u16, 24184u16, 24185u16, 24186u16,
    24187u16, 24188u16, 24189u16, 24190u16, 24191u16, 24192u16, 24193u16, 25099u16, 25100u16,
    25101u16, 25102u16, 25103u16, 25104u16, 25105u16, 25106u16, 25107u16, 25108u16, 25125u16,
    25126u16, 25127u16, 25128u16, 25129u16, 25130u16, 25131u16, 25132u16, 25133u16, 25134u16,
    25135u16, 25136u16, 25137u16, 25138u16, 25139u16, 25140u16, 25141u16, 25142u16, 25143u16,
    25144u16, 25145u16, 25146u16, 25147u16, 25148u16, 25149u16, 25150u16, 25151u16, 25152u16,
    25153u16, 25154u16, 25155u16, 25156u16, 25157u16, 25158u16, 25159u16, 25160u16, 25161u16,
    25162u16, 25163u16, 25164u16, 25165u16, 25166u16, 25167u16, 25168u16, 25169u16, 25170u16,
    25171u16, 25172u16, 25187u16, 33373u16, 33374u16, 33375u16, 33376u16, 33377u16, 33378u16,
    33379u16, 33380u16, 33381u16, 33382u16, 33383u16, 33384u16, 33385u16, 33386u16, 33387u16,
    33388u16, 33389u16, 33390u16, 33391u16, 33392u16, 33393u16, 33394u16, 33395u16, 33396u16,
    33397u16, 33398u16, 33399u16, 33400u16, 33401u16, 33402u16, 33403u16, 33404u16, 33405u16,
    33406u16, 33407u16, 33408u16, 33409u16, 33410u16, 33411u16, 33412u16, 33413u16, 33414u16,
    33415u16, 33416u16, 33417u16, 33418u16, 33419u16, 33420u16, 33421u16, 33422u16, 33423u16,
    33424u16, 33425u16, 33426u16, 33427u16, 33428u16, 33429u16, 33430u16, 33431u16, 33432u16,
    33433u16, 33434u16, 33435u16, 33436u16, 33437u16, 33438u16, 33439u16, 33440u16, 33441u16,
    33442u16, 33443u16, 33444u16, 33445u16, 33446u16, 33447u16, 33448u16, 33449u16, 33450u16,
    33451u16, 33452u16, 33453u16, 33454u16, 33455u16, 33456u16, 33457u16, 33458u16, 33459u16,
    33460u16, 33461u16, 33462u16, 33463u16, 33464u16, 33465u16, 33466u16, 33467u16, 33468u16,
    33469u16, 33470u16, 33471u16, 33472u16, 33473u16, 33474u16, 33475u16, 33476u16, 33477u16,
    33478u16, 33479u16, 33480u16, 33481u16, 33482u16, 33483u16, 33484u16, 33485u16, 33486u16,
    33487u16, 33488u16, 33489u16, 33490u16, 33491u16, 33492u16, 33493u16, 33494u16, 33495u16,
    33496u16, 33497u16, 33498u16, 33499u16, 33500u16, 33501u16, 33502u16, 33503u16, 33504u16,
    33505u16, 33506u16, 33507u16, 33508u16, 33509u16, 33510u16, 33511u16, 33512u16, 33513u16,
    33514u16, 33515u16, 33516u16, 33517u16, 33518u16, 33519u16, 33520u16, 33521u16, 33522u16,
    33523u16, 33524u16, 33525u16, 33526u16, 33527u16, 33528u16, 33529u16, 33530u16, 33531u16,
    33532u16, 33533u16, 33534u16, 33535u16, 33536u16, 33537u16, 33538u16, 33539u16, 33540u16,
    33541u16, 33542u16, 33543u16, 33544u16, 33545u16, 33546u16, 33547u16, 33548u16, 33549u16,
    33550u16, 33551u16, 33552u16, 33553u16, 33554u16, 33555u16, 33556u16, 33557u16, 33558u16,
    33559u16, 33560u16, 33561u16, 33562u16, 33563u16, 33564u16,
];
static STATE_TYPES: &[u16] = &[
    13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16,
    13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16,
    13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16,
    13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16,
    13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16,
    13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16,
    13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16,
    13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16,
    13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16,
    13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 13u16, 17u16, 17u16,
    3u16, 3u16, 3u16, 3u16, 3u16, 3u16, 3u16, 3u16, 7u16, 7u16, 7u16, 7u16, 7u16, 7u16, 7u16, 7u16,
    10u16, 10u16, 10u16, 10u16, 5u16, 5u16, 5u16, 5u16, 5u16, 5u16, 5u16, 5u16, 5u16, 5u16, 5u16,
    5u16, 1u16, 1u16, 1u16, 1u16, 1u16, 1u16, 1u16, 1u16, 0u16, 0u16, 0u16, 0u16, 0u16, 0u16, 0u16,
    0u16, 2u16, 6u16, 12u16, 12u16, 12u16, 12u16, 12u16, 12u16, 12u16, 12u16, 12u16, 12u16, 12u16,
    12u16, 8u16, 8u16, 8u16, 8u16, 8u16, 8u16, 8u16, 8u16, 8u16, 8u16, 8u16, 8u16, 8u16, 8u16,
    8u16, 8u16, 11u16, 9u16, 9u16, 9u16, 9u16, 14u16, 14u16, 14u16, 14u16, 14u16, 14u16, 14u16,
    14u16, 14u16, 14u16, 14u16, 14u16, 14u16, 14u16, 14u16, 14u16, 14u16, 14u16, 14u16, 14u16,
    14u16, 14u16, 14u16, 14u16, 14u16, 14u16, 14u16, 14u16, 14u16, 14u16, 14u16, 14u16, 19u16,
    4u16, 4u16, 4u16, 4u16, 4u16, 4u16, 4u16, 4u16, 4u16, 16u16, 16u16, 16u16, 16u16, 16u16, 16u16,
    16u16, 16u16, 16u16, 16u16, 16u16, 16u16, 16u16, 16u16, 16u16, 16u16, 16u16, 16u16, 16u16,
    16u16, 16u16, 16u16, 16u16, 16u16, 15u16, 15u16, 15u16, 15u16, 15u16, 15u16, 15u16, 15u16,
    15u16, 15u16, 15u16, 15u16, 15u16, 15u16, 15u16, 15u16, 15u16, 15u16, 15u16, 15u16, 15u16,
    15u16, 15u16, 15u16, 18u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16,
    20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16,
    20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16,
    20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16,
    20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16,
    20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16,
    20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16,
    20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16,
    20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16,
    20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16,
    20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16,
    20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16,
    20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16,
    20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16,
    20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16, 20u16,
    20u16,
];
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
