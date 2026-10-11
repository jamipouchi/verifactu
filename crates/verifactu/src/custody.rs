//! The two custody models: the tenant's own certificate, or the
//! vendor's one certificate remitting for every tenant.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::builder::{CertLegs, CertificateSource, SifConfig, Verifactu, VerifactuError};
use crate::clock::Clock;
use crate::domain::chain::{ChainRecord, Predecessor};
use crate::draft::{cancel_draft, emit_draft, EmittedInvoice, InvoiceDraft};
use crate::fiscal::consulta::{self, ConsultaAnswer, ConsultaFilter};
use crate::fiscal::transport::VerifactuTransport;
use crate::fiscal::{Emission, EmissionContext, EmitError, Modality, Obligado, VerifactuEmitter};
use crate::signer::FiscalSigner;

/// The tenant-custody connecting builder ([`Verifactu::tenant`]).
pub struct TenantConnecting<'a, 'r> {
    root: &'r Verifactu,
    cert: CertificateSource<'a>,
    obligado: Option<Obligado>,
    modality: Modality,
    installation: String,
}

impl<'a, 'r> TenantConnecting<'a, 'r> {
    pub(crate) fn new(root: &'r Verifactu, cert: CertificateSource<'a>) -> Self {
        Self {
            root,
            cert,
            obligado: None,
            modality: Modality::Remission,
            installation: String::from("1"),
        }
    }

    #[must_use]
    pub fn obligado(mut self, obligado: Obligado) -> Self {
        self.obligado = Some(obligado);
        self
    }

    /// The tenant's legal modality (remission by default).
    #[must_use]
    pub fn modality(mut self, modality: Modality) -> Self {
        self.modality = modality;
        self
    }

    /// The tenant's `NumeroInstalacion` — AEAT keys record state by
    /// obligado + SIF identity including this (the live-learned 2007
    /// rule), so per-tenant installations are first-class.
    #[must_use]
    pub fn installation(mut self, installation: &str) -> Self {
        installation.clone_into(&mut self.installation);
        self
    }

    /// Parses the certificate once into both legs and builds the
    /// tenant's own pooled emitter. No network traffic happens here.
    ///
    /// # Errors
    /// [`VerifactuError`] per the doors (certificate, identity, wire).
    pub fn connect(self) -> Result<TenantVerifactu, VerifactuError> {
        let obligado = self.obligado.ok_or_else(|| {
            VerifactuError::Identity(String::from("the tenant obligado is required"))
        })?;
        let legs = CertLegs::of(&self.root.clock, self.cert)?;
        let transport = self.root.transport_of(legs.wire, self.modality)?;
        let sif = self.root.sif.for_installation(&self.installation);
        let emitter = VerifactuEmitter::new(self.modality, obligado, sif, legs.signer)
            .map_err(|error| VerifactuError::Identity(error.to_string()))?
            .with_clock(Arc::clone(&self.root.clock))
            .with_transport(transport);
        Ok(TenantVerifactu { emitter })
    }
}

pub struct TenantVerifactu {
    emitter: VerifactuEmitter,
}

impl TenantVerifactu {
    /// The engine face of this connection: ledger owners that seal
    /// their own records submit through the emitter directly
    /// (`engine::VerifactuEmitter::submit`); products never look.
    #[must_use]
    pub fn emitter(&self) -> &VerifactuEmitter {
        &self.emitter
    }

    /// The invoice-level emit (products without their own ledger):
    /// seal the draft onto `prev` (the installation's first record
    /// when `None`) and submit. The return carries the emission AND
    /// the chain cursor — persist `cursor` and hand it back as the
    /// next call's `prev`; it is the only durable state the product
    /// owns.
    ///
    /// A retry after an ambiguous transport failure re-seals the
    /// record at a new instant (a new huella for the same IDFactura):
    /// if the first attempt reached AEAT, the retry comes back as
    /// AEAT's duplicate answer ([`Disposition::DrainWithNote`](crate::response::Disposition::DrainWithNote))
    /// — treat that as accepted, keeping the cursor the accepted
    /// attempt returned.
    ///
    /// # Errors
    /// [`EmitError`] — the draft's gates fire inside, before any byte
    /// serializes.
    pub async fn emit_invoice(
        &self,
        draft: InvoiceDraft,
        prev: Option<&Predecessor>,
    ) -> Result<EmittedInvoice, EmitError> {
        emit_draft(&self.emitter, draft, prev).await
    }

    /// Cancels (anula) a previously emitted invoice: `annulled` is the
    /// invoice's own stored [`Predecessor`] (the cursor its emission
    /// returned), `prev` the chain head to seal onto. The return's
    /// `cursor` is the anulación record — the new chain head.
    ///
    /// # Errors
    /// [`EmitError::InvalidRecord`] when `annulled` names an event
    /// record (only invoices are annullable).
    pub async fn cancel_invoice(
        &self,
        annulled: &Predecessor,
        prev: Option<&Predecessor>,
    ) -> Result<EmittedInvoice, EmitError> {
        cancel_draft(&self.emitter, annulled, prev).await
    }

    /// The read-back of what AEAT stores for this obligado (the
    /// tenant's own lens): every page merged, each answered record
    /// carrying its stored estado + the fields its huella hashes —
    /// [`RegistroConsulta::huella_verifica`](crate::RegistroConsulta::huella_verifica) proves AEAT stores
    /// byte-what-was-sealed.
    ///
    /// # Errors
    /// [`EmitError`] per the consulta pipeline (fault classes included).
    pub async fn consulta(&self, filtro: &ConsultaFilter<'_>) -> Result<ConsultaAnswer, EmitError> {
        self.emitter.consulta(filtro).await
    }
}

/// The vendor-custody connecting builder ([`Verifactu::vendor`]).
pub struct VendorConnecting<'a, 'r> {
    root: &'r Verifactu,
    cert: CertificateSource<'a>,
    representante: Option<Obligado>,
}

impl<'a, 'r> VendorConnecting<'a, 'r> {
    pub(crate) fn new(root: &'r Verifactu, cert: CertificateSource<'a>) -> Self {
        Self {
            root,
            cert,
            representante: None,
        }
    }

    /// MANDATORY in this model: who remits on the tenants' behalf (the
    /// Cabecera `Representante`).
    #[must_use]
    pub fn representante(mut self, representante: Obligado) -> Self {
        self.representante = Some(representante);
        self
    }

    /// Builds the shared-pool vendor connection. ONE transport serves
    /// every tenant; emitters are cached per (tenant, installation,
    /// modality).
    ///
    /// # Errors
    /// [`VerifactuError`] per the doors; [`VerifactuError::Identity`]
    /// when the representante is missing.
    pub fn connect(self) -> Result<VendorVerifactu, VerifactuError> {
        let representante = self.representante.ok_or_else(|| {
            VerifactuError::Identity(String::from(
                "the vendor model demands the representante (the Cabecera block names us)",
            ))
        })?;
        let legs = CertLegs::of(&self.root.clock, self.cert)?;
        let transport = self.root.transport_of(legs.wire, Modality::Remission)?;
        Ok(VendorVerifactu {
            signer: legs.signer,
            transport,
            representante,
            sif: self.root.sif.clone(),
            clock: Arc::clone(&self.root.clock),
            emitters: RwLock::new(HashMap::new()),
        })
    }
}

/// The vendor connection: one certificate, one pool, every tenant.
pub struct VendorVerifactu {
    signer: Arc<dyn FiscalSigner>,
    transport: Arc<dyn VerifactuTransport>,
    representante: Obligado,
    sif: SifConfig,
    clock: Arc<dyn Clock>,
    emitters: RwLock<HashMap<(String, String, Modality), Arc<VerifactuEmitter>>>,
}

impl std::fmt::Debug for VendorVerifactu {
    /// Redacted: never prints signer internals.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VendorVerifactu")
            .field("representante", &self.representante.nif)
            .finish_non_exhaustive()
    }
}

impl VendorVerifactu {
    /// The cached emitter for one tenant, as an [`EmitError`]: emitter
    /// construction only ever fails the identity checks.
    ///
    /// # Panics
    /// On a poisoned emitter-cache lock (a bug, never an input).
    ///
    /// # Errors
    /// [`EmitError::InvalidRecord`] when the tenant identity is
    /// off-contract.
    fn emitter_of(
        &self,
        tenant: &Obligado,
        installation: &str,
        modality: Modality,
    ) -> Result<Arc<VerifactuEmitter>, EmitError> {
        let key = (tenant.nif.clone(), installation.to_owned(), modality);
        if let Some(emitter) = self.emitters.read().expect("emitter cache lock").get(&key) {
            return Ok(Arc::clone(emitter));
        }
        let emitter = VerifactuEmitter::new(
            modality,
            tenant.clone(),
            self.sif.for_installation(installation),
            Arc::clone(&self.signer),
        )?
        .with_clock(Arc::clone(&self.clock))
        .with_representante(self.representante.clone())?
        .with_transport(Arc::clone(&self.transport));
        let emitter = Arc::new(emitter);
        self.emitters
            .write()
            .expect("emitter cache lock")
            .insert(key, Arc::clone(&emitter));
        Ok(emitter)
    }

    /// The invoice-level emit for one tenant (see
    /// [`TenantVerifactu::emit_invoice`], including the retry law).
    ///
    /// # Errors
    /// [`EmitError::InvalidRecord`] when the tenant identity is
    /// off-contract; the rest of [`EmitError`] from the emission itself.
    pub async fn emit_invoice_for(
        &self,
        tenant: &Obligado,
        installation: &str,
        modality: Modality,
        draft: InvoiceDraft,
        prev: Option<&Predecessor>,
    ) -> Result<EmittedInvoice, EmitError> {
        let emitter = self.emitter_of(tenant, installation, modality)?;
        emit_draft(&emitter, draft, prev).await
    }

    /// The anulación for one tenant (see
    /// [`TenantVerifactu::cancel_invoice`]).
    ///
    /// # Errors
    /// [`EmitError::InvalidRecord`] when the tenant identity is
    /// off-contract; the rest of [`EmitError`] from the emission itself.
    pub async fn cancel_invoice_for(
        &self,
        tenant: &Obligado,
        installation: &str,
        modality: Modality,
        annulled: &Predecessor,
        prev: Option<&Predecessor>,
    ) -> Result<EmittedInvoice, EmitError> {
        let emitter = self.emitter_of(tenant, installation, modality)?;
        cancel_draft(&emitter, annulled, prev).await
    }

    /// Emits one envío for one tenant — the ledger-owner's call.
    ///
    /// # Errors
    /// [`EmitError::InvalidRecord`] on emitter construction (the
    /// tenant identity checks); the rest of [`EmitError`] from the
    /// emission itself.
    pub async fn emit_for(
        &self,
        tenant: &Obligado,
        installation: &str,
        modality: Modality,
        pairs: &[(ChainRecord, EmissionContext<'_>)],
    ) -> Result<Emission, EmitError> {
        let emitter = self.emitter_of(tenant, installation, modality)?;
        emitter.submit(pairs).await
    }

    /// The read-back through the APODERADO lens (`IndicadorRepresentante=S`):
    /// AEAT answers the records of `tenant`'s obligation in which we
    /// figure as representante (SI.xsd's own words for the flag).
    ///
    /// The lens answers the AEAT-**registered** representante
    /// relationship, never the emission's `Representante` block —
    /// live-pinned 2026-10-05: a Representante-declared record answers
    /// `SinDatos` (0s/20s/60s) while the cotejo door says `Encontrada`.
    /// With no registration, reconcile through the tenant's own lens
    /// ([`TenantVerifactu::consulta`]).
    ///
    /// # Errors
    /// [`EmitError`] per the consulta pipeline (fault classes included).
    pub async fn consulta_for(
        &self,
        tenant: &Obligado,
        filtro: &ConsultaFilter<'_>,
    ) -> Result<ConsultaAnswer, EmitError> {
        consulta::send_all(&self.transport, tenant, true, filtro).await
    }
}
