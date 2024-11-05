#![cfg_attr(not(feature = "export-abi"), no_main)]
extern crate alloc;


use stylus_sdk::{
    alloy_primitives::{Address, U256, B256},
    contract,
    evm,
    msg,
    prelude::*,
    call::{Call, call},
};
use alloy_sol_types::sol;

sol_interface! {
    interface IERC20 {
        function transferFrom(address from, address to, uint256 amount) external returns (bool);
    }
}

// Define events and errors using Solidity ABI
sol! {
    event NewListing(
        bytes32 indexed id,
        address indexed seller,
        uint256 rate,
        uint256 quantity
    );

    event ListingPaid(
        bytes32 indexed id,
        address indexed seller,
        address indexed buyer,
        uint256 amount,
        uint256 quantity
    );

    // Define errors
    error InvalidListing();
    error InvalidQuantity();
    error InvalidAmount();
    error InvalidSeller();
    error TransferFailed();
    error ListingNotFound();
    error Unauthorized();
}

// Define Status enum
#[derive(Default, Clone, Copy, PartialEq, Eq, StorageType)]
pub enum Status {
    #[default]
    PENDING,
    PAID,
    COMPLETED,
    CANCELLED,
}

// Define Listing struct
#[derive(Default, Clone, StorageType)]
pub struct Listing {
    id: B256,
    seller: Address,
    buyer: Address,
    rate: U256,
    quantity: U256,
    status: Status,
}

// Define storage
sol_storage! {
    #[entrypoint]
    pub struct MerchantPay {
        address USDC;
        mapping(bytes32 => mapping(address => Listing)) listings;
        bytes32[] listing_keys;
        mapping(address => bytes32[]) address_to_listing;
    }
}

// Define the error enum
#[derive(SolidityError)]
pub enum MerchantPayError {
    InvalidListing(InvalidListing),
    InvalidQuantity(InvalidQuantity),
    InvalidAmount(InvalidAmount),
    InvalidSeller(InvalidSeller),
    TransferFailed(TransferFailed),
    ListingNotFound(ListingNotFound),
    Unauthorized(Unauthorized),
}

#[public]
impl MerchantPay {
    pub fn initialize(&mut self, usdc: Address) -> Result<(), MerchantPayError> {
        self.USDC.set(usdc);
        Ok(())
    }

    pub fn add_listing(&mut self, id: B256, rate: U256, quantity: U256) -> Result<(), MerchantPayError> {
        if rate == U256::ZERO || quantity == U256::ZERO {
            return Err(MerchantPayError::InvalidAmount(InvalidAmount{}));
        }

        let listing = Listing {
            id,
            seller: msg::sender(),
            buyer: Address::ZERO,
            rate,
            quantity,
            status: Status::PENDING,
        };

        // Store listing
        let mut seller_listings = self.listings.setter(id);
        seller_listings.setter(msg::sender()).set(listing.clone());

        // Add to listing_keys if new
        let mut is_new_bytes_key = true;
        for i in 0..self.listing_keys.len() {
            if self.listing_keys.get(i) == Some(&id) {
                is_new_bytes_key = false;
                break;
            }
        }
        if is_new_bytes_key {
            self.listing_keys.push(id);
        }

        // Add to address_to_listing if new
        let mut is_new_address_key = true;
        let bytes_keys = self.address_to_listing.getter(msg::sender());
        for i in 0..bytes_keys.len() {
            if bytes_keys.get(i) == Some(&id) {
                is_new_address_key = false;
                break;
            }
        }
        if is_new_address_key {
            self.address_to_listing.setter(msg::sender()).push(id);
        }

        // Emit event
        evm::log(NewListing {
            id,
            seller: msg::sender(),
            rate,
            quantity,
        });
        Ok(())
    }

    pub fn pay_for_listing(
        &mut self, 
        id: B256, 
        seller: Address, 
        quantity: U256, 
        amount: U256
    ) -> Result<(), MerchantPayError> {
        let mut listing_map = self.listings.setter(id);
        let mut listing = listing_map.getter(seller).get();

        // Validate listing
        if listing.status != Status::PENDING && listing.status != Status::PAID {
            return Err(MerchantPayError::InvalidListing(InvalidListing{}));
        }
        
        if quantity > listing.quantity {
            return Err(MerchantPayError::InvalidQuantity(InvalidQuantity{}));
        }

        let price = listing.rate * quantity;
        if amount < price {
            return Err(MerchantPayError::InvalidAmount(InvalidAmount{}));
        }

        // Calculate charge
        let charge = self.deduct_charge(listing.rate);

        // Transfer tokens
        let erc20 = IERC20::new(*self.USDC);
        let config = Call::new_in(self);
        
        // Transfer to seller
        if erc20.transfer_from(config, msg::sender(), seller, price - charge).is_err() {
            return Err(MerchantPayError::TransferFailed(TransferFailed{}));
        }
        
        // Transfer charge
        if erc20.transfer_from(config, msg::sender(), contract::address(), charge).is_err() {
            return Err(MerchantPayError::TransferFailed(TransferFailed{}));
        }

        // Update listing
        listing.buyer = msg::sender();
        listing.quantity -= quantity;
        listing.status = if listing.quantity == U256::ZERO {
            Status::COMPLETED
        } else {
            Status::PAID
        };

        listing_map.setter(seller).set(listing.clone());

        evm::log(ListingPaid {
            id,
            seller,
            buyer: msg::sender(),
            amount,
            quantity,
        });
        Ok(())
    }

    pub fn get_listing(&self, id: B256, seller: Address) -> Result<Listing, MerchantPayError> {
        let listing = self.listings.getter(id).getter(seller).get();
        if listing.seller == Address::ZERO {
            return Err(MerchantPayError::ListingNotFound(ListingNotFound{}));
        }
        Ok(listing)
    }

    pub fn get_all_listings_for_address(&self, seller: Address) -> Result<Vec<Listing>, MerchantPayError> {
        let mut listings = Vec::new();
        let bytes_keys = self.address_to_listing.getter(seller);
        
        for i in 0..bytes_keys.len() {
            if let Some(id) = bytes_keys.get(i) {
                let listing = self.listings.getter(*id).getter(seller).get();
                if listing.seller != Address::ZERO {
                    listings.push(listing);
                }
            }
        }
        
        if listings.is_empty() {
            return Err(MerchantPayError::ListingNotFound(ListingNotFound{}));
        }
        
        Ok(listings)
    }

    fn deduct_charge(&self, amount: U256) -> U256 {
        amount / U256::from(1000) // 0.1%
    }
}