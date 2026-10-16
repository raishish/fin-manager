//! Merchants, counterparties and fake people.
//!
//! Merchant names are real US brands or invented local businesses, as they would appear
//! on a statement. People are always invented, with surnames that are obviously not
//! real (`FAKE_SURNAMES`), so no generated payment can be mistaken for a real person.

/// What a merchant's transactions are, in the ground truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    /// Spending in this category key.
    Cat(&'static str),
    /// Money earned.
    Income,
    /// Money moved to or from one of the user's own accounts that isn't in the ledger.
    Transfer,
}

#[derive(Debug)]
pub struct Merchant {
    pub key: &'static str,
    /// Clean merchant name: the ground truth for merchant clean-up.
    pub name: &'static str,
    pub label: Label,
    /// Card descriptors as the payment processor sends them; one is picked per
    /// transaction. Placeholders: `{s:N}` the persona's usual store number (N digits),
    /// `{r:N}` random letters and digits, `{d:N}` random digits, `{dd}` a restaurant
    /// name (delivery apps). Empty for merchants paid only by ACH.
    pub descriptors: &'static [&'static str],
    /// Fixed location text for online or centrally billed merchants
    /// (`"LOS GATOS CA"`); `None` means the persona's own city.
    pub loc: Option<&'static str>,
    /// Typical price range, in cents.
    pub price: (i64, i64),
    /// ACH company name and entry description, for bank debits and deposits.
    pub ach: Option<(&'static str, &'static str)>,
}

const fn card(
    key: &'static str,
    name: &'static str,
    cat: &'static str,
    descriptors: &'static [&'static str],
    loc: Option<&'static str>,
    lo: i64,
    hi: i64,
) -> Merchant {
    Merchant {
        key,
        name,
        label: Label::Cat(cat),
        descriptors,
        loc,
        price: (lo, hi),
        ach: None,
    }
}

const fn ach(
    key: &'static str,
    name: &'static str,
    label: Label,
    company: &'static str,
    entry: &'static str,
    lo: i64,
    hi: i64,
) -> Merchant {
    Merchant {
        key,
        name,
        label,
        descriptors: &[],
        loc: None,
        price: (lo, hi),
        ach: Some((company, entry)),
    }
}

use Label::{Income, Transfer};

const fn cat(key: &'static str) -> Label {
    Label::Cat(key)
}

/// Every merchant and counterparty the personas use.
#[rustfmt::skip]
pub static MERCHANTS: &[Merchant] = &[
    // Groceries
    card("trader_joes", "Trader Joe's", "food.groceries", &["TRADER JOE'S #{s:3}", "TRADER JOE S #{s:3}"], None, 1800, 9500),
    card("whole_foods", "Whole Foods Market", "food.groceries", &["WHOLEFDS MKT #{s:5}", "WHOLE FOODS MARKET #{s:5}"], None, 2500, 14000),
    card("safeway", "Safeway", "food.groceries", &["SAFEWAY #{s:4}", "SAFEWAY STORE {s:4}"], None, 1500, 9000),
    card("kroger", "Kroger", "food.groceries", &["KROGER #{s:3}", "KROGER {s:3}"], None, 3500, 21000),
    card("aldi", "Aldi", "food.groceries", &["ALDI {s:5}"], None, 2500, 9000),
    card("costco", "Costco", "food.groceries", &["COSTCO WHSE #{s:4}", "COSTCO WHOLESALE #{s:4}"], None, 9000, 34000),
    card("heb", "H-E-B", "food.groceries", &["H-E-B #{s:3}", "HEB #{s:3}"], None, 2500, 16000),
    card("frys", "Fry's Food", "food.groceries", &["FRYS-FOOD-DRG #{s:3}", "FRYS FOOD #{s:3}"], None, 2500, 14000),
    card("sprouts", "Sprouts Farmers Market", "food.groceries", &["SPROUTS FARMERS MAR", "SPROUTS FARMERS MKT #{s:3}"], None, 1500, 7000),
    card("instacart", "Instacart", "food.groceries", &["INSTACART*{r:6}", "IC* INSTACART"], Some("SAN FRANCISCO CA"), 4500, 16000),
    card("pingo_doce", "Pingo Doce", "food.groceries", &["PINGO DOCE LISBOA"], Some("LISBOA PT"), 800, 4500),
    card("lawson", "Lawson", "food.groceries", &["LAWSON {s:5}"], Some("TOKYO JP"), 400, 2500),
    card("seven_eleven_jp", "7-Eleven", "food.groceries", &["7-ELEVEN {s:6}", "SEVEN-ELEVEN JAPAN"], Some("KYOTO JP"), 300, 2000),
    // Restaurants
    card("blue_heron", "Blue Heron Bistro", "food.restaurants", &["TST* BLUE HERON BISTRO", "TST*BLUE HERON BISTRO"], None, 3500, 11000),
    card("casa_verde", "Casa Verde Taqueria", "food.restaurants", &["SQ *CASA VERDE TAQUERIA", "SQ *CASA VERDE TAQUER"], None, 1400, 4500),
    card("sakura_ramen", "Sakura Ramen", "food.restaurants", &["SQ *SAKURA RAMEN"], None, 1800, 5200),
    card("golden_lotus", "Golden Lotus", "food.restaurants", &["GOLDEN LOTUS RESTAURAN", "GOLDEN LOTUS REST"], None, 2500, 8500),
    card("copper_pot", "The Copper Pot", "food.restaurants", &["THE COPPER POT", "COPPER POT"], None, 4500, 13000),
    card("olive_thyme", "Olive & Thyme", "food.restaurants", &["TST* OLIVE AND THYME"], None, 3800, 12000),
    card("olive_garden", "Olive Garden", "food.restaurants", &["OLIVE GARDEN {s:4}", "OLIVE GARDEN #{s:4}"], None, 4500, 11000),
    card("saguaro_grill", "Saguaro Grill", "food.restaurants", &["SAGUARO GRILL", "TST* SAGUARO GRILL"], None, 2800, 7500),
    card("mesquite_diner", "Mesquite Diner", "food.restaurants", &["MESQUITE DINER LLC", "SQ *MESQUITE DINER"], None, 1600, 4200),
    card("lone_star_bbq", "Lone Star Smokehouse", "food.restaurants", &["TST* LONE STAR SMOKEHOU", "LONE STAR SMOKEHOUSE"], None, 2200, 6800),
    card("pastelaria", "Pastelaria São Roque", "food.restaurants", &["PASTELARIA SAO ROQUE"], Some("LISBOA PT"), 450, 1800),
    card("taberna_flores", "Taberna da Rua das Flores", "food.restaurants", &["TABERNA RUA DAS FLORES"], Some("LISBOA PT"), 2500, 7500),
    card("time_out_market", "Time Out Market Lisboa", "food.restaurants", &["TIME OUT MARKET LISBOA"], Some("LISBOA PT"), 1200, 4500),
    card("ichiran", "Ichiran", "food.restaurants", &["ICHIRAN SHIBUYA", "ICHIRAN RAMEN"], Some("TOKYO JP"), 980, 3200),
    card("kyoto_kaiseki", "Gion Hanami Kaiseki", "food.restaurants", &["GION HANAMI KAISEKI"], Some("KYOTO JP"), 8000, 24000),
    card("nyc_deli", "Second Avenue Deli", "food.restaurants", &["2ND AVE DELI"], Some("NEW YORK NY"), 2400, 6500),
    card("nyc_pizza", "Joe's Pizza", "food.restaurants", &["JOES PIZZA BROADWAY"], Some("NEW YORK NY"), 900, 3200),
    card("disney_dining", "Disney Dining", "food.restaurants", &["WDW CONTEMPO CAFE", "WDW SPACE 220", "WDW BE OUR GUEST"], Some("LAKE BUENA VISTA FL"), 4500, 19000),
    card("sd_tacos", "Ocean Beach Tacos", "food.restaurants", &["SQ *OCEAN BEACH TACOS"], Some("SAN DIEGO CA"), 1800, 5500),
    // Coffee
    card("starbucks", "Starbucks", "food.coffee", &["STARBUCKS STORE {s:5}", "STARBUCKS #{s:5}"], None, 450, 1400),
    card("blue_bottle", "Blue Bottle Coffee", "food.coffee", &["SQ *BLUE BOTTLE COFFEE", "BLUE BOTTLE COFFEE {s:3}"], None, 500, 1600),
    card("daily_grind", "The Daily Grind", "food.coffee", &["SQ *THE DAILY GRIND", "SQ *DAILY GRIND"], None, 400, 1300),
    card("dunkin", "Dunkin'", "food.coffee", &["DUNKIN #{s:6}", "DUNKIN {s:6}"], None, 350, 1800),
    card("dutch_bros", "Dutch Bros", "food.coffee", &["DUTCH BROS #{s:4}", "DUTCH BROS COFFEE {s:4}"], None, 450, 1200),
    card("cosmic_cup", "Cosmic Cup Coffee", "food.coffee", &["SQ *COSMIC CUP COFFEE", "SQ *COSMIC CUP"], None, 450, 1500),
    // Fast food
    card("mcdonalds", "McDonald's", "food.fast_food", &["MCDONALD'S F{s:5}", "MCDONALDS F{s:5}"], None, 600, 2600),
    card("chick_fil_a", "Chick-fil-A", "food.fast_food", &["CHICK-FIL-A #0{s:4}", "CHICK-FIL-A #{s:5}"], None, 900, 3800),
    card("chipotle", "Chipotle", "food.fast_food", &["CHIPOTLE {s:4}", "CHIPOTLE ONLINE"], None, 1100, 3500),
    card("panera", "Panera Bread", "food.fast_food", &["PANERA BREAD #{s:6}", "PANERA BREAD {s:6}"], None, 1000, 3600),
    card("taco_bell", "Taco Bell", "food.fast_food", &["TACO BELL #{s:6}", "TACO BELL {s:6}"], None, 600, 2200),
    card("in_n_out", "In-N-Out Burger", "food.fast_food", &["IN N OUT BURGER {s:3}", "IN-N-OUT {s:3}"], None, 700, 2400),
    card("subway", "Subway", "food.fast_food", &["SUBWAY {s:8}", "SUBWAY #{s:5}"], None, 700, 2200),
    // Bars
    card("rusty_anchor", "The Rusty Anchor", "food.bars", &["THE RUSTY ANCHOR", "RUSTY ANCHOR BAR"], None, 1800, 7500),
    card("hopworks", "Hopworks Taproom", "food.bars", &["TST* HOPWORKS TAPROOM", "TST*HOPWORKS TAPROOM"], None, 1500, 6500),
    card("luckys", "Lucky's Tavern", "food.bars", &["LUCKYS TAVERN", "LUCKY'S TAVERN"], None, 1600, 6000),
    // Delivery
    card("doordash", "DoorDash", "food.delivery", &["DOORDASH*{dd}", "DD *DOORDASH {dd}"], Some("SAN FRANCISCO CA"), 2200, 6500),
    card("uber_eats", "Uber Eats", "food.delivery", &["UBER *EATS", "UBER* EATS PENDING"], Some("HELP.UBER.COM CA"), 2000, 6000),
    // Housing
    ach("bayview_rent", "Bayview Property Management", cat("housing.rent"), "BAYVIEW PROP MGMT", "RENT PMT", 0, 0),
    ach("appfolio_rent", "Riverside Flats", cat("housing.rent"), "APPFOLIO RIVERSIDE", "WEB PMTS", 0, 0),
    ach("rocket_mortgage", "Rocket Mortgage", cat("housing.mortgage"), "ROCKET MORTGAGE", "MTG PYMT", 0, 0),
    ach("maple_ridge_hoa", "Maple Ridge HOA", cat("housing.hoa"), "MAPLE RIDGE HOA", "HOA DUES", 0, 0),
    card("home_depot", "The Home Depot", "housing.maintenance", &["THE HOME DEPOT #{s:4}", "THE HOME DEPOT {s:4}", "HOMEDEPOT.COM"], None, 1200, 18000),
    card("lowes", "Lowe's", "housing.maintenance", &["LOWES #0{s:4}*", "LOWE'S #{s:4}"], None, 1500, 16000),
    card("ace_hardware", "Ace Hardware", "housing.maintenance", &["ACE HDWE {s:5}", "ACE HARDWARE #{s:5}"], None, 800, 6500),
    card("buckeye_hvac", "Buckeye Heating & Cooling", "housing.maintenance", &["BUCKEYE HEATING AND C", "BUCKEYE HEATING & COOL"], None, 0, 0),
    card("ikea", "IKEA", "housing.furnishings", &["IKEA {city}", "IKEA US {s:3}"], None, 2500, 60000),
    card("wayfair", "Wayfair", "housing.furnishings", &["WAYFAIR*{r:8}", "WAYFAIR4{d:9}"], Some("800-822-1366 MA"), 4000, 45000),
    // Utilities
    ach("pge", "PG&E", cat("utilities.electricity"), "PGANDE", "WEB ONLINE", 0, 0),
    ach("aep_ohio", "AEP Ohio", cat("utilities.electricity"), "AEP OHIO", "BILL PAY", 0, 0),
    ach("columbia_gas", "Columbia Gas of Ohio", cat("utilities.gas"), "COLUMBIA GAS OH", "UTIL PYMT", 0, 0),
    ach("columbus_water", "City of Columbus Utilities", cat("utilities.water_trash"), "CITY OF COLUMBUS", "UTILITIES", 0, 0),
    ach("austin_energy", "Austin Energy", cat("utilities.electricity"), "CITY OF AUSTIN", "UTILITY", 0, 0),
    ach("tep", "Tucson Electric Power", cat("utilities.electricity"), "TUCSON ELECTRIC", "TEP PAYMT", 0, 0),
    ach("southwest_gas", "Southwest Gas", cat("utilities.gas"), "SOUTHWEST GAS", "SWGAS PMT", 0, 0),
    ach("tucson_water", "Tucson Water", cat("utilities.water_trash"), "CITY OF TUCSON", "WATER BILL", 0, 0),
    card("comcast", "Xfinity", "utilities.internet", &["COMCAST CALIFORNIA", "COMCAST XFINITY"], Some("800-COMCAST CA"), 0, 0),
    card("spectrum", "Spectrum", "utilities.internet", &["SPECTRUM", "CHARTER SPECTRUM"], Some("855-707-7328 MO"), 0, 0),
    card("google_fiber", "Google Fiber", "utilities.internet", &["GOOGLE *FIBER", "GOOGLE *Google Fiber"], Some("g.co/payhelp# CA"), 0, 0),
    card("cox", "Cox Communications", "utilities.internet", &["COX COMM TUCSON", "COX ARIZONA COMM SVCS"], Some("888-269-9693 AZ"), 0, 0),
    card("verizon", "Verizon", "utilities.phone", &["VZWRLSS*APOCC VISB", "VERIZON WRLS P{d:5}-01"], Some("800-922-0204 FL"), 0, 0),
    card("tmobile", "T-Mobile", "utilities.phone", &["TMOBILE*AUTO PAY", "T-MOBILE AUTO PAY"], Some("800-937-8997 WA"), 0, 0),
    card("mint_mobile", "Mint Mobile", "utilities.phone", &["MINT MOBILE", "MINT MOBILE {d:10}"], Some("MINTMOBILE.COM CA"), 0, 0),
    card("att_wireless", "AT&T", "utilities.phone", &["ATT*BILL PAYMENT", "AT&T*BILL PAYMENT"], Some("800-331-0500 TX"), 0, 0),
    // Transport
    card("shell", "Shell", "transport.fuel", &["SHELL OIL {s:11}", "SHELL SERVICE STATION"], None, 2800, 7200),
    card("chevron", "Chevron", "transport.fuel", &["CHEVRON {s:7}", "CHEVRON {s:4}"], None, 3000, 7500),
    card("speedway", "Speedway", "transport.fuel", &["SPEEDWAY {s:5}", "SPEEDWAY {s:5} {city}"], None, 3000, 7000),
    card("costco_gas", "Costco Gas", "transport.fuel", &["COSTCO GAS #{s:4}", "COSTCO GAS {s:4}"], None, 3500, 8000),
    card("clipper", "Clipper", "transport.transit", &["CLIPPER SYSTEMS", "CLIPPER CARD AUTOLOAD"], Some("CONCORD CA"), 2000, 5000),
    card("mta_nyc", "MTA New York City Transit", "transport.transit", &["MTA*NYCT PAYGO"], Some("NEW YORK NY"), 290, 290),
    card("metro_lisboa", "Metro Lisboa", "transport.transit", &["METRO LISBOA"], Some("LISBOA PT"), 165, 1200),
    card("uber", "Uber", "transport.rideshare", &["UBER *TRIP", "UBER   *TRIP {r:4}", "UBER* TRIP"], Some("HELP.UBER.COM CA"), 900, 4200),
    card("lyft", "Lyft", "transport.rideshare", &["LYFT   *RIDE", "LYFT *RIDE {r:3}"], Some("SAN FRANCISCO CA"), 900, 3800),
    card("sfmta", "SFMTA Parking", "transport.parking_tolls", &["SFMTA PAYBYPHONE", "SFMTA METER"], Some("SAN FRANCISCO CA"), 300, 1800),
    card("parkmobile", "ParkMobile", "transport.parking_tolls", &["PARKMOBILE", "PARKMOBILE {d:6}"], Some("877-727-5718 GA"), 250, 1500),
    card("jiffy_lube", "Jiffy Lube", "transport.auto_maintenance", &["JIFFY LUBE #{s:4}"], None, 7500, 13000),
    card("discount_tire", "Discount Tire", "transport.auto_maintenance", &["DISCOUNT TIRE {s:4}", "DISCOUNT TIRE STORE"], None, 0, 0),
    card("firestone", "Firestone", "transport.auto_maintenance", &["FIRESTONE {s:6}", "FIRESTONE COMPLETE AUTO"], None, 0, 0),
    ach("toyota_financial", "Toyota Financial Services", cat("transport.auto_loan"), "TOYOTA PAYMENTS", "AUTO PYMT", 0, 0),
    // Shopping
    card("amazon", "Amazon", "shopping.general", &["AMZN Mktp US*{r:9}", "Amazon.com*{r:9}", "AMAZON MKTPL*{r:9}"], Some("Amzn.com/bill WA"), 900, 9500),
    card("amazon_prime", "Amazon", "shopping.general", &["Amazon Prime*{r:9}"], Some("Amzn.com/bill WA"), 0, 0),
    card("target", "Target", "shopping.general", &["TARGET 000{s:4}", "TARGET        000{s:4}", "TARGET.COM  *"], None, 1200, 14000),
    card("walmart", "Walmart", "shopping.general", &["WAL-MART #{s:4}", "WM SUPERCENTER #{s:4}", "WALMART.COM"], None, 1500, 12000),
    card("old_navy", "Old Navy", "shopping.clothing", &["OLD NAVY US {s:4}", "OLDNAVY.COM"], None, 2000, 9000),
    card("uniqlo", "Uniqlo", "shopping.clothing", &["UNIQLO {city}", "UNIQLO USA"], None, 2500, 12000),
    card("nordstrom_rack", "Nordstrom Rack", "shopping.clothing", &["NORDSTROM RACK #{s:4}", "NORDSTROMRACK.COM"], None, 3000, 16000),
    card("tj_maxx", "TJ Maxx", "shopping.clothing", &["TJMAXX #{s:4}", "T J MAXX #{s:4}"], None, 1500, 9000),
    card("best_buy", "Best Buy", "shopping.electronics", &["BEST BUY {s:8}", "BESTBUY.COM {d:9}"], None, 1500, 45000),
    card("apple_store", "Apple Store", "shopping.electronics", &["APPLE STORE #R{s:3}", "APPLE.COM/US"], None, 0, 0),
    card("rei", "REI", "shopping.sporting_goods", &["REI #{s:2} {city}", "REI.COM"], None, 2500, 22000),
    card("dicks", "Dick's Sporting Goods", "shopping.sporting_goods", &["DICK'S CLOTHING&SPOR", "DICKS SPORTING GOODS"], None, 2500, 18000),
    card("michaels", "Michaels", "shopping.hobbies", &["MICHAELS STORES {s:4}"], None, 1200, 6500),
    card("barnes_noble", "Barnes & Noble", "shopping.hobbies", &["BARNES & NOBLE #{s:4}", "B&N.COM"], None, 1200, 6000),
    card("carters", "Carter's", "family.kids_supplies", &["CARTERS #{s:4}", "CARTERS.COM"], None, 2000, 9000),
    card("childrens_place", "The Children's Place", "family.kids_supplies", &["THE CHILDRENS PLACE {s:4}", "CHILDRENSPLACE.COM"], None, 1800, 7500),
    // Entertainment
    card("netflix", "Netflix", "entertainment.streaming", &["NETFLIX.COM", "NETFLIX.COM {d:3}"], Some("LOS GATOS CA"), 0, 0),
    card("hulu", "Hulu", "entertainment.streaming", &["HLU*HULU {d:10}-U", "HULU {d:10}"], Some("HULU.COM/BILL CA"), 0, 0),
    card("disney_plus", "Disney+", "entertainment.streaming", &["DISNEY PLUS", "DISNEYPLUS"], Some("888-905-7888 CA"), 0, 0),
    card("max", "Max", "entertainment.streaming", &["MAX.COM", "HBO MAX"], Some("NEW YORK NY"), 0, 0),
    card("spotify", "Spotify", "entertainment.music", &["SPOTIFY USA", "Spotify USA"], Some("877-7781161 NY"), 0, 0),
    card("apple_music", "Apple", "entertainment.music", &["APPLE.COM/BILL"], Some("866-712-7753 CA"), 0, 0),
    card("steam", "Steam", "entertainment.games", &["STEAMGAMES.COM {d:4}", "STEAM PURCHASE"], Some("425-889-9642 WA"), 500, 6000),
    card("playstation", "PlayStation Network", "entertainment.games", &["PLAYSTATION NETWORK", "SONY PLAYSTATION"], Some("800-345-7669 CA"), 1000, 7000),
    card("ticketmaster", "Ticketmaster", "entertainment.events", &["TICKETMASTER", "TM *TICKETMASTER"], Some("800-653-8000 CA"), 4500, 24000),
    card("amc", "AMC Theatres", "entertainment.events", &["AMC {s:4} ONLINE", "AMC THEATRES {s:4}"], None, 1500, 6000),
    card("wdw_tickets", "Walt Disney World", "entertainment.events", &["WDW TICKETS", "WALT DISNEY WORLD TIX"], Some("LAKE BUENA VISTA FL"), 0, 0),
    card("botanical_gardens", "Tucson Botanical Gardens", "entertainment.events", &["TUCSON BOTANICAL GARD"], None, 0, 0),
    // Health
    card("cvs", "CVS Pharmacy", "health.pharmacy", &["CVS/PHARMACY #{s:5}", "CVS/PHARM {s:5}"], None, 800, 7500),
    card("walgreens", "Walgreens", "health.pharmacy", &["WALGREENS #{s:4}", "WALGREENS STORE {s:4}"], None, 600, 6500),
    card("bayside_medical", "Bayside Medical Group", "health.medical", &["BAYSIDE MEDICAL GRP", "BAYSIDE MED GRP"], None, 2500, 4500),
    card("riverside_peds", "Riverside Pediatrics", "health.medical", &["RIVERSIDE PEDIATRICS"], None, 2500, 4000),
    card("desert_sky_cardio", "Desert Sky Cardiology", "health.medical", &["DESERT SKY CARDIOLOGY", "DESERT SKY CARDIO PC"], None, 4000, 9500),
    card("quest", "Quest Diagnostics", "health.medical", &["QUEST DIAGNOSTICS INC", "QUEST DIAG"], Some("866-697-8378 NJ"), 1800, 14000),
    card("costco_hearing", "Costco Hearing Aid Center", "health.medical", &["COSTCO HEARING AID {s:4}"], None, 0, 0),
    card("bright_smile", "Bright Smile Dental", "health.dental", &["BRIGHT SMILE DENTAL", "BRIGHT SMILE DENTAL PC"], None, 4500, 18000),
    card("smile_ortho", "Smile Bright Orthodontics", "health.dental", &["SMILE BRIGHT ORTHO"], None, 0, 0),
    card("warby_parker", "Warby Parker", "health.vision", &["WARBY PARKER", "WARBYPARKER.COM"], Some("NEW YORK NY"), 9500, 29500),
    card("clear_view", "Clear View Optometry", "health.vision", &["CLEAR VIEW OPTOMETRY"], None, 8000, 32000),
    card("planet_fitness", "Planet Fitness", "health.fitness", &["PLANET FITNESS", "PF CLUB FEES"], Some("844-880-7180 NH"), 0, 0),
    card("classpass", "ClassPass", "health.fitness", &["CLASSPASS* MONTHLY", "CLASSPASS*{r:6}"], Some("CLASSPASS.COM NY"), 0, 0),
    card("equinox", "Equinox", "health.fitness", &["EQUINOX #{s:3}", "EQUINOX FITNESS CLUBS"], Some("NEW YORK NY"), 0, 0),
    ach("ymca", "YMCA of Central Ohio", cat("health.fitness"), "YMCA CENTRAL OHIO", "MEMBERSHIP", 0, 0),
    card("black_swan_yoga", "Black Swan Yoga", "health.fitness", &["SQ *BLACK SWAN YOGA", "BLACK SWAN YOGA"], None, 1500, 12000),
    // Personal care
    card("great_clips", "Great Clips", "personal_care.hair_beauty", &["GREAT CLIPS #{s:4}", "GREAT CLIPS {s:4}"], None, 1800, 3500),
    card("luxe_hair", "Luxe Hair Studio", "personal_care.hair_beauty", &["SQ *LUXE HAIR STUDIO", "SQ *LUXE HAIR"], None, 4500, 14000),
    card("sephora", "Sephora", "personal_care.hair_beauty", &["SEPHORA {s:4}", "SEPHORA.COM"], None, 1800, 9500),
    card("massage_envy", "Massage Envy", "personal_care.spa", &["MASSAGE ENVY {s:4}", "MASSAGE ENVY - {city}"], None, 7500, 13000),
    card("fluff_fold", "Fluff & Fold Laundromat", "personal_care.laundry", &["SQ *FLUFF N FOLD", "SQ *FLUFF AND FOLD"], None, 1600, 4200),
    card("sunshine_cleaners", "Sunshine Cleaners", "personal_care.laundry", &["SUNSHINE CLEANERS", "SUNSHINE DRY CLEANERS"], None, 1200, 4500),
    // Travel
    card("united", "United Airlines", "travel.flights", &["UNITED {d:13}", "UNITED AIRLINES {d:13}"], Some("800-932-2732 TX"), 0, 0),
    card("delta", "Delta Air Lines", "travel.flights", &["DELTA AIR {d:13}", "DELTA AIR LINES"], Some("DELTA.COM CA"), 0, 0),
    card("southwest", "Southwest Airlines", "travel.flights", &["SOUTHWES {d:13}", "SOUTHWEST AIRLINES"], Some("800-435-9792 TX"), 0, 0),
    card("tap_air", "TAP Air Portugal", "travel.flights", &["TAP AIR PORTUGAL {d:10}"], Some("LISBOA PT"), 0, 0),
    card("jal", "Japan Airlines", "travel.flights", &["JAPAN AIRLINES {d:13}", "JAL {d:13}"], Some("TOKYO JP"), 0, 0),
    card("hilton", "Hilton", "travel.lodging", &["HILTON GARDEN INN {city}", "HILTON HOTELS {d:6}"], None, 0, 0),
    card("airbnb", "Airbnb", "travel.lodging", &["AIRBNB * {r:10}", "AIRBNB  HM{r:8}"], Some("415-800-5959 CA"), 0, 0),
    card("disney_resort", "Disney Resort Hotels", "travel.lodging", &["DISNEY RESORT HOTEL", "WDW POP CENTURY RESORT"], Some("LAKE BUENA VISTA FL"), 0, 0),
    card("granvia_kyoto", "Hotel Granvia Kyoto", "travel.lodging", &["HOTEL GRANVIA KYOTO"], Some("KYOTO JP"), 0, 0),
    card("hertz", "Hertz", "travel.car_rental", &["HERTZ RENT-A-CAR", "HERTZ CAR RENTAL {d:6}"], None, 0, 0),
    card("jr_central", "JR Central", "travel.other", &["JR CENTRAL SHINKANSEN", "JR TOKAI TICKET"], Some("TOKYO JP"), 2000, 15000),
    card("tsa_precheck", "TSA PreCheck", "travel.other", &["TSA PRECHECK", "IDEMIA TSA PRECHECK"], Some("855-347-8371 CA"), 0, 0),
    // Family
    ach("little_acorns", "Little Acorns Learning Center", cat("family.childcare"), "LITTLE ACORNS LLC", "TUITION", 0, 0),
    card("ymca_camp", "YMCA Summer Camp", "family.childcare", &["YMCA OF CENTRAL OHIO", "YMCA CAMP REGISTRATION"], None, 0, 0),
    card("youth_soccer", "Columbus Youth Soccer", "family.kids_activities", &["COLUMBUS YOUTH SOCCER", "CYS REGISTRATION"], None, 0, 0),
    card("sunrise_swim", "Sunrise Swim School", "family.kids_activities", &["SQ *SUNRISE SWIM SCHOOL", "SQ *SUNRISE SWIM"], None, 0, 0),
    card("coursera", "Coursera", "family.education", &["COURSERA.ORG", "COURSERA {r:6}"], Some("MOUNTAIN VIEW CA"), 0, 0),
    // Pets
    card("chewy", "Chewy", "pets.supplies", &["CHEWY.COM", "CHEWY INC"], Some("800-672-4399 FL"), 0, 0),
    card("petco", "Petco", "pets.supplies", &["PETCO {s:4}", "PETCO ANIMAL SUPPLIES {s:4}"], None, 1500, 8500),
    card("banfield", "Banfield Pet Hospital", "pets.vet", &["BANFIELD PET HOSP #{s:4}", "BANFIELD #{s:4}"], None, 6500, 32000),
    card("desert_paws", "Desert Paws Veterinary", "pets.vet", &["DESERT PAWS VETERINARY", "DESERT PAWS VET CLINIC"], None, 8000, 26000),
    // Insurance
    card("lemonade", "Lemonade", "insurance.home", &["LEMONADE I* RENTERS", "LEMONADE INSURANCE CO"], Some("NEW YORK NY"), 0, 0),
    ach("state_farm_auto", "State Farm", cat("insurance.auto"), "STATE FARM RO 27", "SFPP", 0, 0),
    ach("state_farm_home", "State Farm", cat("insurance.home"), "STATE FARM RO 08", "SFPP", 0, 0),
    ach("progressive", "Progressive", cat("insurance.auto"), "PROGRESSIVE INS", "PREM PYMT", 0, 0),
    ach("haven_life", "Haven Life", cat("insurance.life"), "HAVEN LIFE INS", "PREMIUM", 0, 0),
    ach("bcbs_tx", "Blue Cross Blue Shield of Texas", cat("insurance.health"), "BCBSTX", "PREMIUM", 0, 0),
    ach("uhc_medigap", "UnitedHealthcare", cat("insurance.health"), "UNITEDHEALTHCARE", "INS PREM", 0, 0),
    // Financial
    ach("nelnet", "Nelnet", cat("financial.loan_payment"), "NELNET", "STUDENT LN", 0, 0),
    // Taxes and government
    ach("irs_payment", "IRS", cat("taxes.income"), "IRS", "USATAXPYMT", 0, 0),
    ach("pima_county", "Pima County Treasurer", cat("taxes.property"), "PIMA COUNTY TREAS", "TAXPAYMENT", 0, 0),
    card("ca_dmv", "California DMV", "taxes.government_fees", &["CA DMV FEE", "DMV CA INTERNET"], Some("SACRAMENTO CA"), 0, 0),
    card("ohio_bmv", "Ohio BMV", "taxes.government_fees", &["OH BMV ONLINE", "OHIO BMV REG"], Some("COLUMBUS OH"), 0, 0),
    // Giving
    card("red_cross", "American Red Cross", "giving.charity", &["AMERICAN RED CROSS", "REDCROSS.ORG"], Some("800-733-2767 DC"), 0, 0),
    card("wikimedia", "Wikimedia Foundation", "giving.charity", &["WIKIMEDIA*DONATION", "WIKIMEDIA FOUNDATION"], Some("SAN FRANCISCO CA"), 0, 0),
    card("msf", "Doctors Without Borders", "giving.charity", &["DOCTORS WITHOUT BORDE", "MSF USA DONATION"], Some("212-679-6800 NY"), 0, 0),
    card("flowers", "1-800-Flowers", "giving.gifts", &["1-800-FLOWERS.COM", "1800FLOWERS*{d:8}"], Some("800-468-1141 NY"), 5500, 11000),
    // Services
    card("apple_icloud", "Apple", "services.software", &["APPLE.COM/BILL"], Some("866-712-7753 CA"), 0, 0),
    card("adobe", "Adobe", "services.software", &["ADOBE *CREATIVE CLD", "ADOBE  *ADOBE"], Some("408-536-6000 CA"), 0, 0),
    card("figma", "Figma", "services.software", &["FIGMA", "FIGMA.COM"], Some("SAN FRANCISCO CA"), 0, 0),
    card("github", "GitHub", "services.software", &["GITHUB, INC.", "GITHUB INC"], Some("HTTPSGITHUB.C CA"), 0, 0),
    card("notion", "Notion", "services.software", &["NOTION LABS, INC.", "NOTION.SO"], Some("SAN FRANCISCO CA"), 0, 0),
    card("dropbox", "Dropbox", "services.software", &["DROPBOX*{r:10}", "DROPBOX"], Some("DROPBOX.COM CA"), 0, 0),
    card("google_workspace", "Google Workspace", "services.software", &["GOOGLE *GSUITE_samf", "GOOGLE*WORKSPACE samf"], Some("cc@google.com CA"), 0, 0),
    card("zoom", "Zoom", "services.software", &["ZOOM.US 888-799-9666", "ZOOM.COM"], Some("SAN JOSE CA"), 0, 0),
    card("capital_factory", "Capital Factory", "services.professional", &["CAPITAL FACTORY", "CAPITALFACTORY COWORK"], None, 0, 0),
    card("hr_block", "H&R Block", "services.professional", &["H&R BLOCK", "HRB ONLINE TAX PROD"], Some("800-472-5625 MO"), 4500, 12000),
    card("usps", "USPS", "services.shipping", &["USPS PO {s:10}", "USPS.COM CLICKNSHIP"], None, 500, 4500),
    card("ups_store", "The UPS Store", "services.shipping", &["THE UPS STORE {s:4}", "THE UPS STORE #{s:4}"], None, 1200, 6500),
    // Paid by paper check: the statement shows only the check number.
    card("melody_lane", "Melody Lane Piano Studio", "family.kids_activities", &["CHECK"], None, 0, 0),
    card("green_thumb", "Green Thumb Lawn Care", "housing.maintenance", &["CHECK"], None, 0, 0),
    card("st_marks", "St. Mark's Church", "giving.charity", &["CHECK"], None, 0, 0),
    card("sunny_days", "Sunny Days Cleaning", "housing.maintenance", &["CHECK"], None, 0, 0),
    card("sun_valley_roofing", "Sun Valley Roofing", "housing.maintenance", &["CHECK"], None, 0, 0),
    // Income
    ach("northwind", "Northwind Analytics", Income, "NORTHWIND ANALYT", "PAYROLL", 0, 0),
    ach("contoso_health", "Contoso Health", Income, "CONTOSO HEALTH SYS", "DIR DEP", 0, 0),
    ach("fabrikam_schools", "Fabrikam Schools", Income, "FABRIKAM ISD", "PAYROLL", 0, 0),
    ach("tailspin", "Tailspin Toys", Income, "TAILSPIN TOYS INC", "ACH PMT", 0, 0),
    ach("wide_world", "Wide World Importers", Income, "WIDE WORLD IMPORTE", "VENDOR PMT", 0, 0),
    ach("stripe", "Stripe", Income, "STRIPE", "TRANSFER", 0, 0),
    ach("upwork", "Upwork", Income, "UPWORK ESCROW INC", "EDI PYMNTS", 0, 0),
    ach("ssa", "Social Security", Income, "SSA TREAS 310", "XXSOC SEC", 0, 0),
    ach("asrs", "Arizona State Retirement System", Income, "AZ STATE RETIREMEN", "PENSION", 0, 0),
    ach("irs_refund", "IRS", cat("taxes.income"), "IRS TREAS 310", "TAX REF", 0, 0),
    ach("ohio_refund", "Ohio Department of Taxation", cat("taxes.income"), "STATE OF OHIO", "TAX REFUND", 0, 0),
    // Own accounts that aren't in the ledger
    ach("robinhood", "Robinhood", Transfer, "ROBINHOOD", "FUNDS", 0, 0),
    ach("ohio_529", "CollegeAdvantage 529", Transfer, "OHIO TOS-529", "CONTRIB", 0, 0),
    ach("vanguard", "Vanguard", Transfer, "VANGUARD", "BUY INVESTMENT", 0, 0),
    ach("fidelity", "Fidelity", Transfer, "FID BKG SVC LLC", "MONEYLINE", 0, 0),
];

/// Looks up a merchant by key. Panics on a typo, pointing at the calling line
/// (the message doesn't repeat the key: no merchant names in panics).
#[track_caller]
pub fn merchant(key: &str) -> &'static Merchant {
    match MERCHANTS.iter().find(|m| m.key == key) {
        Some(m) => m,
        None => panic!("unknown merchant key; see catalog::MERCHANTS"),
    }
}

/// Whether another merchant with a different label bills under one of the same
/// descriptors (Apple Music and iCloud are both `APPLE.COM/BILL`), so the description
/// alone can't tell them apart.
pub fn shares_descriptor(m: &Merchant) -> bool {
    MERCHANTS.iter().any(|o| {
        o.key != m.key
            && o.label != m.label
            && o.descriptors.iter().any(|d| m.descriptors.contains(d))
    })
}

/// Restaurant names that delivery apps put in the descriptor.
pub static DELIVERY_RESTAURANTS: &[&str] = &[
    "THAI BASIL",
    "SWEETGREEN",
    "PAPA JOHNS",
    "SHAKE SHACK",
    "PHO HOA",
    "CURRY UP NOW",
    "CHIPOTLE",
    "PANDA EXPRESS",
    "WINGSTOP",
    "HALAL GUYS",
];

pub static FAKE_FIRST_NAMES: &[&str] = &[
    "Quinn", "Jordan", "Riley", "Avery", "Morgan", "Casey", "Taylor", "Jamie", "Rowan", "Skyler",
    "Devon", "Emerson", "Harper", "Kendall", "Logan", "Parker", "Reese", "Sage",
];

/// Surnames chosen to be obviously invented, so a generated P2P payment can never be
/// read as a payment to a real person.
pub static FAKE_SURNAMES: &[&str] = &[
    "Placeholder",
    "Exampleton",
    "Testworth",
    "Fakerson",
    "Sampleby",
    "Mockridge",
    "Dummyfield",
    "Notareal",
    "Fictionsen",
    "Synthwell",
    "Madeupson",
    "Lorem",
    "Ipsumworth",
    "Pretendley",
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn keys_are_unique() {
        let mut seen = BTreeSet::new();
        for m in MERCHANTS {
            assert!(seen.insert(m.key), "duplicate merchant key {}", m.key);
            assert!(
                m.ach.is_some() || !m.descriptors.is_empty(),
                "{} has no descriptor",
                m.key
            );
            assert!(m.price.0 <= m.price.1, "{} price range", m.key);
        }
    }
}
