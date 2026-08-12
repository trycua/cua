use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, GenericArgument, PathArguments, Type, parse_macro_input};

#[proc_macro_derive(UniffiBuilder, attributes(uniffi_builder))]
pub fn derive_uniffi_builder(input: TokenStream) -> TokenStream {
    expand(parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "UniffiBuilder does not support generic records",
        ));
    }
    let record = input.ident;
    let builder = format_ident!("{record}Builder");
    let error = input
        .attrs
        .iter()
        .find(|attribute| attribute.path().is_ident("uniffi_builder"))
        .ok_or_else(|| syn::Error::new_spanned(&record, "missing #[uniffi_builder(ErrorType)]"))?
        .parse_args::<syn::Path>()?;
    let fields = match input.data {
        Data::Struct(data) => match data.fields {
            Fields::Named(fields) => fields.named,
            fields => {
                return Err(syn::Error::new_spanned(
                    fields,
                    "UniffiBuilder requires named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                &record,
                "UniffiBuilder requires a struct",
            ));
        }
    };

    let mut state_fields = Vec::new();
    let mut initializers = Vec::new();
    let mut setters = Vec::new();
    let mut builds = Vec::new();
    for field in fields {
        let name = field.ident.expect("named field");
        if name == "new" || name == "build" {
            return Err(syn::Error::new_spanned(
                name,
                "UniffiBuilder fields cannot be named `new` or `build`",
            ));
        }
        let ty = field.ty;
        let optional = option_inner(&ty)?;
        let setter_ty = optional.unwrap_or(&ty).clone();
        state_fields.push(quote! { #name: ::std::option::Option<#setter_ty> });
        initializers.push(quote! { #name: ::std::option::Option::None });
        setters.push((name.clone(), setter_ty));
        if optional.is_some() {
            builds.push(quote! { #name: self.#name.clone() });
        } else {
            builds.push(quote! {
                #name: self.#name.clone().ok_or_else(|| #error::missing(
                    stringify!(#record), stringify!(#name),
                ))?
            });
        }
    }

    let field_names = setters.iter().map(|(name, _)| name).collect::<Vec<_>>();
    let setter_methods = setters.iter().map(|(setter_name, setter_ty)| {
        let assignments = field_names.iter().map(|field_name| {
            if *field_name == setter_name {
                quote! { #field_name: ::std::option::Option::Some(value) }
            } else {
                quote! { #field_name: self.#field_name.clone() }
            }
        });
        quote! {
            pub fn #setter_name(&self, value: #setter_ty) -> ::std::sync::Arc<Self> {
                ::std::sync::Arc::new(Self { #(#assignments,)* })
            }
        }
    });

    Ok(quote! {
        #[derive(uniffi::Object)]
        pub struct #builder { #(#state_fields,)* }

        #[uniffi::export]
        impl #builder {
            #[uniffi::constructor]
            pub fn new() -> ::std::sync::Arc<Self> {
                ::std::sync::Arc::new(Self { #(#initializers,)* })
            }

            #(#setter_methods)*

            pub fn build(&self) -> ::std::result::Result<#record, #error> {
                ::std::result::Result::Ok(#record { #(#builds,)* })
            }
        }
    })
}

fn option_inner(ty: &Type) -> syn::Result<Option<&Type>> {
    let Type::Path(path) = ty else {
        return Ok(None);
    };
    let segments = path.path.segments.iter().collect::<Vec<_>>();
    let Some(segment) = segments.last() else {
        return Ok(None);
    };
    if segment.ident != "Option" {
        return Ok(None);
    }
    if path.qself.is_some() {
        return Err(syn::Error::new_spanned(
            ty,
            "UniffiBuilder optional fields must use Option<T>, std::option::Option<T>, or core::option::Option<T>",
        ));
    }
    let supported_path = matches!(segments.as_slice(), [option] if option.ident == "Option")
        || matches!(segments.as_slice(), [root, module, option]
            if (root.ident == "std" || root.ident == "core")
                && module.ident == "option"
                && option.ident == "Option");
    if !supported_path {
        return Err(syn::Error::new_spanned(
            ty,
            "UniffiBuilder optional fields must use Option<T>, std::option::Option<T>, or core::option::Option<T>",
        ));
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Ok(None);
    };
    match arguments.args.iter().collect::<Vec<_>>().as_slice() {
        [GenericArgument::Type(inner)] => Ok(Some(inner)),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::expand;
    use syn::{DeriveInput, parse_quote};

    #[test]
    fn rejects_generic_records() {
        let input: DeriveInput = parse_quote! {
            #[uniffi_builder(crate::BuildError)]
            struct Generic<T> { value: T }
        };
        assert_eq!(
            expand(input).unwrap_err().to_string(),
            "UniffiBuilder does not support generic records"
        );
    }

    #[test]
    fn rejects_reserved_field_names() {
        let input: DeriveInput = parse_quote! {
            #[uniffi_builder(crate::BuildError)]
            struct Reserved { build: String }
        };
        assert_eq!(
            expand(input).unwrap_err().to_string(),
            "UniffiBuilder fields cannot be named `new` or `build`"
        );
    }

    #[test]
    fn rejects_unnamed_fields() {
        let input: DeriveInput = parse_quote! {
            #[uniffi_builder(crate::BuildError)]
            struct Tuple(String);
        };
        assert_eq!(
            expand(input).unwrap_err().to_string(),
            "UniffiBuilder requires named fields"
        );
    }

    #[test]
    fn accepts_supported_option_paths() {
        for input in [
            parse_quote! {
                #[uniffi_builder(crate::BuildError)]
                struct Short { value: Option<String> }
            },
            parse_quote! {
                #[uniffi_builder(crate::BuildError)]
                struct Std { value: std::option::Option<String> }
            },
            parse_quote! {
                #[uniffi_builder(crate::BuildError)]
                struct Core { value: core::option::Option<String> }
            },
        ] {
            expand(input).unwrap();
        }
    }

    #[test]
    fn rejects_ambiguous_option_paths() {
        let input: DeriveInput = parse_quote! {
            #[uniffi_builder(crate::BuildError)]
            struct Ambiguous { value: custom::Option<String> }
        };
        assert_eq!(
            expand(input).unwrap_err().to_string(),
            "UniffiBuilder optional fields must use Option<T>, std::option::Option<T>, or core::option::Option<T>"
        );
    }

    #[test]
    fn rejects_qualified_self_option_paths() {
        let input: DeriveInput = parse_quote! {
            #[uniffi_builder(crate::BuildError)]
            struct Qualified { value: <Custom as Trait>::Option<String> }
        };
        assert_eq!(
            expand(input).unwrap_err().to_string(),
            "UniffiBuilder optional fields must use Option<T>, std::option::Option<T>, or core::option::Option<T>"
        );
    }
}
