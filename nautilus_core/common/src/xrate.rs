// -------------------------------------------------------------------------------------------------
//  Copyright (C) 2015-2024 Nautech Systems Pty Ltd. All rights reserved.
//  https://nautechsystems.io
//
//  Licensed under the GNU Lesser General Public License Version 3.0 (the "License");
//  You may not use this file except in compliance with the License.
//  You may obtain a copy of the License at https://www.gnu.org/licenses/lgpl-3.0.en.html
//
//  Unless required by applicable law or agreed to in writing, software
//  distributed under the License is distributed on an "AS IS" BASIS,
//  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//  See the License for the specific language governing permissions and
//  limitations under the License.
// -------------------------------------------------------------------------------------------------

// ****************************************************************************
// The design of exchange rate calculations needs to be revisited,
// as its not efficient to be allocating so many structures and doing so many recalculations"
// ****************************************************************************

//! Exchange rate calculations between currencies.
//!
//! An exchange rate is the value of one asset versus that of another.
use std::collections::{HashMap, HashSet};

use itertools::Itertools;
use nautilus_core::correctness::{check_equal_usize, check_map_not_empty, FAILED};
use nautilus_model::{enums::PriceType, identifiers::Symbol, types::currency::Currency};
use rust_decimal::Decimal;
use ustr::Ustr;

// TODO: Improve efficiency: Check Top Comment
/// Returns the calculated exchange rate for the given price type using the
/// given dictionary of bid and ask quotes.
pub fn get_exchange_rate(
    from_currency: Currency,
    to_currency: Currency,
    price_type: PriceType,
    quotes_bid: HashMap<Symbol, Decimal>,
    quotes_ask: HashMap<Symbol, Decimal>,
) -> Decimal {
    check_map_not_empty(&quotes_bid, stringify!(quotes_bid)).expect(FAILED);
    check_map_not_empty(&quotes_ask, stringify!(quotes_ask)).expect(FAILED);
    check_equal_usize(
        quotes_bid.len(),
        quotes_ask.len(),
        "quotes_bid.len()",
        "quotes_ask.len()",
    )
    .expect(FAILED);

    if from_currency == to_currency {
        return Decimal::ONE;
    }

    let calculation_quotes = match price_type {
        PriceType::Bid => quotes_bid,
        PriceType::Ask => quotes_ask,
        PriceType::Mid => quotes_bid
            .iter()
            .map(|(k, v)| {
                let ask = quotes_ask.get(k).unwrap_or(v);
                (*k, (v + ask) / Decimal::TWO)
            })
            .collect(),
        _ => {
            panic!(
                "Cannot calculate exchange rate for PriceType: {:?}",
                price_type
            );
        }
    };

    let mut codes = HashSet::new();
    let mut exchange_rates: HashMap<Ustr, HashMap<Ustr, Decimal>> = HashMap::new();

    // Build quote table
    for (symbol, quote) in calculation_quotes.iter() {
        // Split symbol into currency pairs
        let pieces: Vec<&str> = symbol.as_str().split('/').collect();
        let code_lhs = Ustr::from(pieces[0]);
        let code_rhs = Ustr::from(pieces[1]);

        codes.insert(code_lhs);
        codes.insert(code_rhs);

        // Initialize currency dictionaries if they don't exist
        exchange_rates.entry(code_lhs).or_default();
        exchange_rates.entry(code_rhs).or_default();

        // Add base rates
        if let Some(rates_lhs) = exchange_rates.get_mut(&code_lhs) {
            rates_lhs.insert(code_lhs, Decimal::ONE);
            rates_lhs.insert(code_rhs, *quote);
        }
        if let Some(rates_rhs) = exchange_rates.get_mut(&code_rhs) {
            rates_rhs.insert(code_rhs, Decimal::ONE);
        }
    }

    // Generate possible currency pairs from all symbols
    let code_perms: Vec<(Ustr, Ustr)> = codes
        .iter()
        .cartesian_product(codes.iter())
        .filter(|(a, b)| a != b)
        .map(|(a, b)| (*a, *b))
        .collect();

    // Calculate currency inverses
    for (perm0, perm1) in code_perms.iter() {
        // First direction: perm0 -> perm1
        let rate_0_to_1 = exchange_rates
            .get(perm0)
            .and_then(|rates| rates.get(perm1))
            .copied();

        if let Some(rate) = rate_0_to_1 {
            if let Some(xrate_perm1) = exchange_rates.get_mut(perm1) {
                if !xrate_perm1.contains_key(perm0) {
                    xrate_perm1.insert(*perm0, Decimal::ONE / rate);
                }
            }
        }

        // Second direction: perm1 -> perm0
        let rate_1_to_0 = exchange_rates
            .get(perm1)
            .and_then(|rates| rates.get(perm0))
            .copied();

        if let Some(rate) = rate_1_to_0 {
            if let Some(xrate_perm0) = exchange_rates.get_mut(perm0) {
                if !xrate_perm0.contains_key(perm1) {
                    xrate_perm0.insert(*perm1, Decimal::ONE / rate);
                }
            }
        }
    }

    // Check if we already have the rate
    if let Some(quotes) = exchange_rates.get(&from_currency.code) {
        if let Some(&rate) = quotes.get(&to_currency.code) {
            return rate;
        }
    }

    // Calculate remaining exchange rates through common currencies
    for (perm0, perm1) in code_perms.iter() {
        // Skip if rate already exists
        if exchange_rates
            .get(perm1)
            .map_or(false, |rates| rates.contains_key(perm0))
        {
            continue;
        }

        // Search for common currency
        for code in codes.iter() {
            // First check: rates through common currency
            let rates_through_common = {
                let rates_perm0 = exchange_rates.get(perm0);
                let rates_perm1 = exchange_rates.get(perm1);

                match (rates_perm0, rates_perm1) {
                    (Some(rates0), Some(rates1)) => {
                        if let (Some(&rate1), Some(&rate2)) = (rates0.get(code), rates1.get(code)) {
                            Some((rate1, rate2))
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            };

            // Second check: rates from code's perspective
            let rates_from_code = if rates_through_common.is_none() {
                if let Some(rates_code) = exchange_rates.get(code) {
                    if let (Some(&rate1), Some(&rate2)) =
                        (rates_code.get(perm0), rates_code.get(perm1))
                    {
                        Some((rate1, rate2))
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            // Apply the found rates if any
            if let Some((common_rate1, common_rate2)) = rates_through_common.or(rates_from_code) {
                // Insert forward rate
                if let Some(rates_perm1) = exchange_rates.get_mut(perm1) {
                    rates_perm1.insert(*perm0, common_rate2 / common_rate1);
                }

                // Insert inverse rate
                if let Some(rates_perm0) = exchange_rates.get_mut(perm0) {
                    if !rates_perm0.contains_key(perm1) {
                        rates_perm0.insert(*perm1, common_rate1 / common_rate2);
                    }
                }
            }
        }
    }

    let xrate = exchange_rates
        .get(&from_currency.code)
        .and_then(|quotes| quotes.get(&to_currency.code))
        .copied()
        .unwrap_or(Decimal::ZERO);

    xrate
}
